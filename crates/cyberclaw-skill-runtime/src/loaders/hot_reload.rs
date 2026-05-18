//! Skill 热重载监控器
//!
//! 使用文件系统监控实现 Skill 的热重载功能

use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::loaders::UnifiedSkillLoader;
use crate::runtime::SkillRuntime;

/// 热重载事件类型
#[derive(Debug, Clone)]
pub enum ReloadEvent {
    /// Skill 文件被修改
    Modified(PathBuf),
    /// Skill 文件被创建
    Created(PathBuf),
    /// Skill 文件被删除
    Deleted(PathBuf),
}

/// 热重载监控器
pub struct HotReloadWatcher {
    /// 文件系统监控器
    _watcher: Box<dyn Watcher + Send>,
    /// 事件发送通道（用于测试注入与内部扩展）
    #[allow(dead_code)]
    event_tx: mpsc::UnboundedSender<ReloadEvent>,
    /// 事件接收通道
    event_rx: Arc<RwLock<mpsc::UnboundedReceiver<ReloadEvent>>>,
    /// Skill 加载器
    loader: Arc<UnifiedSkillLoader>,
    /// 防抖延迟（毫秒）
    debounce_ms: u64,
}

impl HotReloadWatcher {
    /// 创建新的热重载监控器
    ///
    /// # 参数
    ///
    /// * `loader` - Skill 加载器
    /// * `debounce_ms` - 防抖延迟（毫秒），用于避免频繁的文件变更触发
    ///
    /// # 返回
    ///
    /// 返回监控器实例
    pub fn new(loader: Arc<UnifiedSkillLoader>, debounce_ms: u64) -> Self {
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        // 创建文件系统监控器
        let callback_event_tx = event_tx.clone();
        let watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            match res {
                Ok(event) => {
                    // 处理文件系统事件
                    match event.kind {
                        EventKind::Modify(_) => {
                            for path in event.paths {
                                let _ = callback_event_tx.send(ReloadEvent::Modified(path));
                            }
                        }
                        EventKind::Create(_) => {
                            for path in event.paths {
                                let _ = callback_event_tx.send(ReloadEvent::Created(path));
                            }
                        }
                        EventKind::Remove(_) => {
                            for path in event.paths {
                                let _ = callback_event_tx.send(ReloadEvent::Deleted(path));
                            }
                        }
                        _ => {}
                    }
                }
                Err(e) => {
                    error!(error = %e, "文件监控错误");
                }
            }
        })
        .expect("Failed to create file watcher");

        // 包装 watcher 为 Box<dyn Watcher + Send>
        let boxed_watcher: Box<dyn Watcher + Send> = Box::new(watcher);

        Self {
            _watcher: boxed_watcher,
            event_tx,
            event_rx: Arc::new(RwLock::new(event_rx)),
            loader,
            debounce_ms,
        }
    }

    /// 监控指定目录
    ///
    /// # 参数
    ///
    /// * `skill_dirs` - Skill 目录列表
    ///
    /// # 返回
    ///
    /// 成功返回 Ok(())，失败返回错误
    pub fn watch(&mut self, skill_dirs: Vec<PathBuf>) -> Result<(), notify::Error> {
        for dir in skill_dirs {
            info!(path = %dir.display(), "开始监控 Skill 目录");
            self._watcher.watch(&dir, RecursiveMode::Recursive)?;
        }
        Ok(())
    }

    /// 启动热重载监控循环
    ///
    /// # 参数
    ///
    /// * `runtime` - Skill 运行时，用于重新注册 Skill
    pub async fn start_watching<R: SkillRuntime + 'static>(
        &self,
        runtime: Arc<R>,
    ) -> tokio::task::JoinHandle<()> {
        let event_rx = self.event_rx.clone();
        let loader = self.loader.clone();
        let debounce_duration = Duration::from_millis(self.debounce_ms);

        tokio::spawn(async move {
            let mut rx = event_rx.write().await;
            let mut pending_paths: Vec<PathBuf> = Vec::new();

            loop {
                // 创建新的防抖定时器（如果有待处理路径）
                let sleep_fut = if !pending_paths.is_empty() {
                    Some(tokio::time::sleep(debounce_duration))
                } else {
                    None
                };

                tokio::select! {
                    // 接收文件变更事件
                    Some(event) = rx.recv() => {
                        match event {
                            ReloadEvent::Modified(path) | ReloadEvent::Created(path) => {
                                debug!(path = %path.display(), "检测到文件变更");

                                // 处理 Skill 相关文件，兼容不同平台 watcher 上报粒度差异：
                                // - 有的后端上报具体文件（SKILL.md）
                                // - 有的后端只上报目录变更
                                // 因此除了文件名判断，还允许能反向定位到 Skill 根目录的路径。
                                if Self::is_skill_file(&path) || Self::find_skill_root(&path).is_some() {
                                    pending_paths.push(path);
                                }
                            }
                            ReloadEvent::Deleted(path) => {
                                warn!(path = %path.display(), "Skill 文件被删除");

                                // 尝试定位 Skill 根目录（文件删除时目录可能仍存在）
                                if let Some(skill_dir) = Self::find_skill_root(&path) {
                                    // 从 loader 缓存中清除，避免后续重新加载时命中旧缓存
                                    loader.evict_cache(&skill_dir).await;

                                    // 根据路径推导 skill_id（与加载时的规则一致：取目录名）
                                    if let Some(dir_name) = skill_dir.file_name().and_then(|n| n.to_str()) {
                                        match cyberclaw_core::ids::SkillId::from_string(dir_name.to_string()) {
                                            Ok(skill_id) => {
                                                match runtime.unregister_skill(&skill_id).await {
                                                    Ok(true) => {
                                                        info!(
                                                            skill_id = %skill_id,
                                                            path = %skill_dir.display(),
                                                            "Skill 已卸载"
                                                        );
                                                    }
                                                    Ok(false) => {
                                                        debug!(
                                                            skill_id = %skill_id,
                                                            path = %skill_dir.display(),
                                                            "Skill 卸载：未找到已注册的 skill，可能尚未加载"
                                                        );
                                                    }
                                                    Err(e) => {
                                                        error!(
                                                            skill_id = %skill_id,
                                                            error = %e,
                                                            "Skill 卸载失败"
                                                        );
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                warn!(
                                                    dir_name = %dir_name,
                                                    error = %e,
                                                    "无法从目录名推导 SkillId，跳过卸载"
                                                );
                                            }
                                        }
                                    }
                                } else {
                                    debug!(
                                        path = %path.display(),
                                        "已删除文件不属于任何 Skill 目录，跳过卸载"
                                    );
                                }
                            }
                        }
                    }

                    // 防抖计时器到期
                    _ = async {
                        match sleep_fut {
                            Some(sleep) => sleep.await,
                            None => std::future::pending::<()>().await,
                        }
                    } => {
                        // 处理所有待重载的路径
                        for path in pending_paths.drain(..) {
                            if let Some(skill_dir) = Self::find_skill_root(&path) {
                                info!(path = %skill_dir.display(), "重新加载 Skill");

                                match loader.reload_skill(&skill_dir).await {
                                    Ok(loaded) => {
                                        // 重新注册到运行时
                                        runtime
                                            .register_skill(loaded.id.clone(), loaded.handler)
                                            .await;

                                        info!(
                                            skill_id = %loaded.id,
                                            skill_name = %loaded.metadata.name,
                                            "Skill 热重载成功"
                                        );
                                    }
                                    Err(e) => {
                                        error!(
                                            path = %skill_dir.display(),
                                            error = %e,
                                            "Skill 重载失败"
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        })
    }

    /// 检查是否为 Skill 相关文件
    fn is_skill_file(path: &Path) -> bool {
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        matches!(
            file_name,
            "SKILL.md" | "manifest.yaml" | "manifest.yml" | "skill.toml"
        )
    }

    /// 查找 Skill 根目录
    ///
    /// 向上遍历目录树，找到包含 Skill 标识文件的目录
    fn find_skill_root(path: &Path) -> Option<PathBuf> {
        // 从“最可能的目录”开始检查：
        // - 目录事件：直接从该目录检查
        // - 文件事件：从文件所在目录检查
        let mut current = if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent()?.to_path_buf()
        };

        // 向上遍历最多 6 层（包含起始目录）
        for _ in 0..6 {
            if current.join("SKILL.md").exists()
                || current.join("manifest.yaml").exists()
                || current.join("manifest.yml").exists()
                || current.join("skill.toml").exists()
            {
                return Some(current);
            }
            if !current.pop() {
                break;
            }
        }

        None
    }

    #[cfg(test)]
    fn emit_test_event(&self, event: ReloadEvent) {
        let _ = self.event_tx.send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::MinimalSkillRuntime;
    use tempfile::TempDir;
    use tokio::fs;
    use tokio::time::sleep;

    #[test]
    fn test_is_skill_file() {
        assert!(HotReloadWatcher::is_skill_file(&PathBuf::from(
            "/path/to/SKILL.md"
        )));
        assert!(HotReloadWatcher::is_skill_file(&PathBuf::from(
            "/path/to/manifest.yaml"
        )));
        assert!(HotReloadWatcher::is_skill_file(&PathBuf::from(
            "/path/to/manifest.yml"
        )));
        assert!(HotReloadWatcher::is_skill_file(&PathBuf::from(
            "/path/to/skill.toml"
        )));
        assert!(!HotReloadWatcher::is_skill_file(&PathBuf::from(
            "/path/to/random.txt"
        )));
        assert!(!HotReloadWatcher::is_skill_file(&PathBuf::from(
            "/path/to/README.md"
        )));
    }

    #[tokio::test]
    async fn test_find_skill_root() {
        let temp_dir = TempDir::new().unwrap();
        let skill_dir = temp_dir.path().join("my-skill");
        fs::create_dir(&skill_dir).await.unwrap();

        // 创建 SKILL.md
        fs::write(skill_dir.join("SKILL.md"), "test").await.unwrap();

        // 创建子目录和文件
        let sub_dir = skill_dir.join("scripts");
        fs::create_dir(&sub_dir).await.unwrap();
        let script_file = sub_dir.join("main.py");
        fs::write(&script_file, "print('hello')").await.unwrap();

        // 从子文件查找根目录
        let found = HotReloadWatcher::find_skill_root(&script_file);
        assert!(found.is_some());
        assert_eq!(found.unwrap(), skill_dir);
    }

    #[tokio::test]
    async fn test_hot_reload_watcher_creation() {
        let loader = Arc::new(UnifiedSkillLoader::new());
        let _watcher = HotReloadWatcher::new(loader, 100);

        // 如果创建成功，测试通过
    }

    #[tokio::test]
    async fn test_watch_directory() {
        let temp_dir = TempDir::new().unwrap();
        let skill_dir = temp_dir.path().join("watch-test");
        fs::create_dir(&skill_dir).await.unwrap();

        let loader = Arc::new(UnifiedSkillLoader::new());
        let mut watcher = HotReloadWatcher::new(loader, 100);

        let result = watcher.watch(vec![skill_dir]);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_hot_reload_integration() {
        let temp_dir = TempDir::new().unwrap();
        let skill_dir = temp_dir.path().join("hot-reload-skill");
        fs::create_dir(&skill_dir).await.unwrap();

        // 创建初始 Skill
        let initial_content = r#"---
name: Hot Reload Test
version: 1.0.0
description: Initial version
---
# Hot Reload Test
"#;

        fs::write(skill_dir.join("SKILL.md"), initial_content)
            .await
            .unwrap();

        let loader = Arc::new(UnifiedSkillLoader::new());
        let runtime = Arc::new(MinimalSkillRuntime::new());

        // 加载初始 Skill
        let initial = loader.load_skill(&skill_dir).await.unwrap();
        runtime
            .register_skill(initial.id.clone(), initial.handler)
            .await;

        // 创建热重载监控器
        let mut watcher = HotReloadWatcher::new(loader.clone(), 50);
        watcher.watch(vec![skill_dir.clone()]).unwrap();

        // 启动监控
        let _handle = watcher.start_watching(runtime.clone()).await;

        // 修改 Skill 文件
        let updated_content = r#"---
name: Hot Reload Test
version: 2.0.0
description: Updated version
---
# Hot Reload Test - Updated
"#;

        // 等待文件监视器完成初始化后再写入
        sleep(Duration::from_millis(100)).await;
        fs::write(skill_dir.join("SKILL.md"), updated_content)
            .await
            .unwrap();

        // 测试注入事件，避免不同平台/沙箱下文件系统通知差异导致假失败
        watcher.emit_test_event(ReloadEvent::Modified(skill_dir.join("SKILL.md")));

        // 事件驱动等待：轮询直到文件内容反映 2.0.0，带 5 秒超时保护
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let reloaded = loader.load_skill(&skill_dir).await.unwrap();
            if reloaded.metadata.version == "2.0.0" {
                assert_eq!(reloaded.metadata.description, "Updated version");
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!("Timeout: hot-reload did not complete within 5 seconds");
            }
            sleep(Duration::from_millis(20)).await;
        }
    }
}
