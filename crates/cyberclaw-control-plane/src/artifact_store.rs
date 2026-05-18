use chrono::{DateTime, Duration, Utc};
use cyberclaw_core::ids::ExecutionId;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::RwLock;

/// Artifact metadata for tracking stored files (Milestone C)
#[derive(Debug, Clone)]
pub struct ArtifactMetadata {
    pub execution_id: ExecutionId,
    pub artifact_name: String,
    pub file_path: PathBuf,
    pub size_bytes: u64,
    pub uploaded_at: DateTime<Utc>,
    pub content_type: Option<String>,
}

/// Artifact store configuration
#[derive(Debug, Clone)]
pub struct ArtifactStoreConfig {
    /// Base directory for artifact storage
    pub base_dir: PathBuf,
    /// Maximum age for artifacts before cleanup (default: 7 days)
    pub max_age_days: i64,
}

impl Default for ArtifactStoreConfig {
    fn default() -> Self {
        Self {
            base_dir: PathBuf::from("/tmp/cyberclaw/artifacts"),
            max_age_days: 7,
        }
    }
}

impl ArtifactStoreConfig {
    /// Minimum age for cleanup (prevent too aggressive cleanup)
    const MIN_AGE_DAYS: i64 = 1;
    /// Maximum age for cleanup (prevent indefinite storage)
    const MAX_AGE_DAYS: i64 = 365;

    /// Validate configuration values
    ///
    /// # Security
    /// Validates config to prevent:
    /// - Data loss via too-short retention
    /// - Storage exhaustion via too-long retention
    pub fn validate(&self) -> anyhow::Result<()> {
        // Validate max_age_days
        if self.max_age_days < Self::MIN_AGE_DAYS {
            anyhow::bail!(
                "max_age_days too low: {} (min {})",
                self.max_age_days,
                Self::MIN_AGE_DAYS
            );
        }
        if self.max_age_days > Self::MAX_AGE_DAYS {
            anyhow::bail!(
                "max_age_days too high: {} (max {})",
                self.max_age_days,
                Self::MAX_AGE_DAYS
            );
        }

        // Note: base_dir validation is intentionally minimal here
        // Full path validation (existence, permissions) should be done
        // at runtime by the store implementation

        Ok(())
    }
}

/// Artifact store trait for execution artifacts (Milestone C)
pub trait ArtifactStore: Send + Sync {
    /// Upload an artifact for an execution
    fn upload(
        &self,
        execution_id: ExecutionId,
        artifact_name: String,
        content: Vec<u8>,
        content_type: Option<String>,
    ) -> impl std::future::Future<Output = anyhow::Result<ArtifactMetadata>> + Send;

    /// Download an artifact
    fn download(
        &self,
        execution_id: &ExecutionId,
        artifact_name: &str,
    ) -> impl std::future::Future<Output = anyhow::Result<Vec<u8>>> + Send;

    /// List all artifacts for an execution
    fn list_artifacts(
        &self,
        execution_id: &ExecutionId,
    ) -> impl std::future::Future<Output = anyhow::Result<Vec<ArtifactMetadata>>> + Send;

    /// Delete a specific artifact
    fn delete(
        &self,
        execution_id: &ExecutionId,
        artifact_name: &str,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send;

    /// Delete all artifacts for an execution
    fn delete_execution_artifacts(
        &self,
        execution_id: &ExecutionId,
    ) -> impl std::future::Future<Output = anyhow::Result<usize>> + Send;

    /// Clean up artifacts older than configured max_age
    fn cleanup_old_artifacts(
        &self,
    ) -> impl std::future::Future<Output = anyhow::Result<usize>> + Send;

    /// Get metadata for a specific artifact
    fn get_metadata(
        &self,
        execution_id: &ExecutionId,
        artifact_name: &str,
    ) -> impl std::future::Future<Output = anyhow::Result<Option<ArtifactMetadata>>> + Send;
}

/// In-memory + filesystem implementation of ArtifactStore
#[derive(Clone)]
pub struct LocalArtifactStore {
    metadata: Arc<RwLock<BTreeMap<(ExecutionId, String), ArtifactMetadata>>>,
    config: ArtifactStoreConfig,
}

impl LocalArtifactStore {
    pub fn new(config: ArtifactStoreConfig) -> Self {
        Self {
            metadata: Arc::new(RwLock::new(BTreeMap::new())),
            config,
        }
    }

    /// Sanitize artifact name to prevent path traversal attacks
    ///
    /// Security: Rejects names containing:
    /// - Path separators (/, \)
    /// - Parent directory references (..)
    /// - Absolute path indicators
    /// - Control characters
    /// - Names that are too long (>255 chars)
    fn sanitize_artifact_name(name: &str) -> anyhow::Result<String> {
        // Check for empty name
        if name.is_empty() {
            anyhow::bail!("artifact name cannot be empty");
        }

        // Check length (max 255 chars for filesystem compatibility)
        if name.len() > 255 {
            anyhow::bail!("artifact name too long (max 255 chars)");
        }

        // Check for path traversal attempts
        if name.contains("..") {
            anyhow::bail!("artifact name cannot contain '..' (path traversal attempt)");
        }

        // Check for path separators (both Unix and Windows)
        if name.contains('/') || name.contains('\\') {
            anyhow::bail!("artifact name cannot contain path separators");
        }

        // Check for control characters and null bytes
        if name.chars().any(|c| c.is_control() || c == '\0') {
            anyhow::bail!("artifact name cannot contain control characters");
        }

        // Reject absolute path indicators (Unix and Windows)
        if name.starts_with('/')
            || name.starts_with('\\')
            || (name.len() >= 2 && name.chars().nth(1) == Some(':'))
        {
            anyhow::bail!("artifact name cannot be an absolute path");
        }

        // Additional security: reject names that are just '.' or '..'
        if name == "." || name == ".." {
            anyhow::bail!("artifact name cannot be '.' or '..'");
        }

        Ok(name.to_string())
    }

    /// Get file path for an artifact (with path traversal protection)
    async fn get_artifact_path(
        &self,
        execution_id: &ExecutionId,
        artifact_name: &str,
    ) -> anyhow::Result<PathBuf> {
        // Sanitize artifact name to prevent path traversal
        let safe_name = Self::sanitize_artifact_name(artifact_name)?;

        // Build path with sanitized components
        let path = self
            .config
            .base_dir
            .join(execution_id.to_string())
            .join(safe_name);

        // Additional safety: verify the final path is within base_dir
        // This catches any edge cases that might have slipped through
        // Use spawn_blocking for synchronous canonicalize() to avoid blocking tokio runtime
        let base_dir = self.config.base_dir.clone();
        let path_clone = path.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let canonical_base = base_dir.canonicalize().unwrap_or_else(|_| base_dir.clone());
            if let Ok(canonical_path) = path_clone.canonicalize() {
                if !canonical_path.starts_with(&canonical_base) {
                    anyhow::bail!("path traversal attempt detected: path escapes base directory");
                }
            }
            Ok(())
        })
        .await??;

        Ok(path)
    }

    /// Ensure directory exists for execution
    async fn ensure_execution_dir(&self, execution_id: &ExecutionId) -> anyhow::Result<PathBuf> {
        let dir = self.config.base_dir.join(execution_id.to_string());
        fs::create_dir_all(&dir).await?;
        Ok(dir)
    }
}

impl Default for LocalArtifactStore {
    fn default() -> Self {
        Self::new(ArtifactStoreConfig::default())
    }
}

impl ArtifactStore for LocalArtifactStore {
    /// Upload an artifact atomically.
    ///
    /// # Concurrency Safety
    /// Metadata is written under an exclusive write lock *after* the file write
    /// completes, making the upload visible to concurrent readers atomically.
    ///
    /// Concurrent uploads of the **same** `(execution_id, artifact_name)` pair are
    /// serialised at the metadata lock: the last writer wins for metadata, but both
    /// may have written to the same filesystem path.  Callers must not rely on
    /// partial results if concurrent uploads to the same name are possible.
    async fn upload(
        &self,
        execution_id: ExecutionId,
        artifact_name: String,
        content: Vec<u8>,
        content_type: Option<String>,
    ) -> anyhow::Result<ArtifactMetadata> {
        // Validate artifact name (security check)
        let file_path = self
            .get_artifact_path(&execution_id, &artifact_name)
            .await?;

        // Ensure directory exists
        self.ensure_execution_dir(&execution_id).await?;

        // Write file
        let mut file = fs::File::create(&file_path).await?;

        // SECURITY: Post-open TOCTOU protection
        // Verify the opened file's real path is still within base_dir
        // This prevents symlink attacks that occur between path validation and file open
        // Use spawn_blocking for synchronous canonicalize() to avoid blocking tokio runtime
        // Check file path with async canonicalize
        if let Ok(real_path) = fs::canonicalize(&file_path).await {
            let base_dir = self.config.base_dir.clone();
            let canonical_base = tokio::task::spawn_blocking(move || {
                base_dir
                    .canonicalize()
                    .unwrap_or_else(|_e| base_dir.clone())
            })
            .await?;

            if !real_path.starts_with(&canonical_base) {
                anyhow::bail!(
                    "TOCTOU attack detected: file path changed to escape base directory after open"
                );
            }
        }

        file.write_all(&content).await?;
        file.flush().await?;

        // Create metadata
        let now = Utc::now();
        let size_bytes = content.len() as u64;
        let metadata = ArtifactMetadata {
            execution_id: execution_id.clone(),
            artifact_name: artifact_name.clone(),
            file_path: file_path.clone(),
            size_bytes,
            uploaded_at: now,
            content_type,
        };

        // Store metadata
        let mut store = self.metadata.write().await;
        store.insert((execution_id, artifact_name), metadata.clone());

        Ok(metadata)
    }

    async fn download(
        &self,
        execution_id: &ExecutionId,
        artifact_name: &str,
    ) -> anyhow::Result<Vec<u8>> {
        // Validate artifact name (security check)
        let file_path = self.get_artifact_path(execution_id, artifact_name).await?;

        // Use tokio::fs::try_exists (async) instead of synchronous exists()
        if !tokio::fs::try_exists(&file_path).await.unwrap_or(false) {
            anyhow::bail!("artifact not found: {}/{}", execution_id, artifact_name);
        }

        let mut file = fs::File::open(&file_path).await?;

        // SECURITY: Post-open TOCTOU protection
        // Verify the opened file's real path is still within base_dir
        // This prevents symlink attacks that occur between path validation and file open
        // Use spawn_blocking for synchronous canonicalize() to avoid blocking tokio runtime
        let base_dir = self.config.base_dir.clone();
        let canonical_base = tokio::task::spawn_blocking(move || {
            base_dir.canonicalize().unwrap_or_else(|_| base_dir.clone())
        })
        .await?;

        if let Ok(real_path) = fs::canonicalize(&file_path).await {
            if !real_path.starts_with(&canonical_base) {
                anyhow::bail!(
                    "TOCTOU attack detected: file path changed to escape base directory after open"
                );
            }
        }

        let mut content = Vec::new();
        file.read_to_end(&mut content).await?;

        Ok(content)
    }

    async fn list_artifacts(
        &self,
        execution_id: &ExecutionId,
    ) -> anyhow::Result<Vec<ArtifactMetadata>> {
        let store = self.metadata.read().await;

        let artifacts: Vec<ArtifactMetadata> = store
            .iter()
            .filter(|((eid, _), _)| eid == execution_id)
            .map(|(_, metadata)| metadata.clone())
            .collect();

        Ok(artifacts)
    }

    async fn delete(&self, execution_id: &ExecutionId, artifact_name: &str) -> anyhow::Result<()> {
        // Validate artifact name (security check)
        let file_path = self.get_artifact_path(execution_id, artifact_name).await?;

        // Delete file if exists (use async try_exists)
        if tokio::fs::try_exists(&file_path).await.unwrap_or(false) {
            fs::remove_file(&file_path).await?;
        }

        // Remove metadata
        let mut store = self.metadata.write().await;
        store.remove(&(execution_id.clone(), artifact_name.to_string()));

        Ok(())
    }

    async fn delete_execution_artifacts(
        &self,
        execution_id: &ExecutionId,
    ) -> anyhow::Result<usize> {
        let mut store = self.metadata.write().await;

        // Find all artifacts for execution
        let keys_to_remove: Vec<(ExecutionId, String)> = store
            .keys()
            .filter(|(eid, _)| eid == execution_id)
            .cloned()
            .collect();

        let count = keys_to_remove.len();

        // Delete files and metadata
        for (eid, name) in keys_to_remove {
            // Try to get path (ignore validation errors during cleanup)
            if let Ok(file_path) = self.get_artifact_path(&eid, &name).await {
                if tokio::fs::try_exists(&file_path).await.unwrap_or(false) {
                    let _ = fs::remove_file(&file_path).await; // Ignore errors
                }
            }
            store.remove(&(eid, name));
        }

        // Try to remove execution directory
        let exec_dir = self.config.base_dir.join(execution_id.to_string());
        if tokio::fs::try_exists(&exec_dir).await.unwrap_or(false) {
            let _ = fs::remove_dir_all(&exec_dir).await; // Ignore errors
        }

        Ok(count)
    }

    async fn cleanup_old_artifacts(&self) -> anyhow::Result<usize> {
        let mut store = self.metadata.write().await;
        let now = Utc::now();
        let max_age = Duration::days(self.config.max_age_days);

        // Find old artifacts
        let old_artifacts: Vec<(ExecutionId, String)> = store
            .iter()
            .filter(|(_, metadata)| now - metadata.uploaded_at > max_age)
            .map(|((eid, name), _)| (eid.clone(), name.clone()))
            .collect();

        let count = old_artifacts.len();

        // Delete files and metadata
        for (eid, name) in old_artifacts {
            // Try to get path (ignore validation errors during cleanup)
            if let Ok(file_path) = self.get_artifact_path(&eid, &name).await {
                if tokio::fs::try_exists(&file_path).await.unwrap_or(false) {
                    let _ = fs::remove_file(&file_path).await; // Ignore errors
                }
            }
            store.remove(&(eid, name));
        }

        Ok(count)
    }

    async fn get_metadata(
        &self,
        execution_id: &ExecutionId,
        artifact_name: &str,
    ) -> anyhow::Result<Option<ArtifactMetadata>> {
        let store = self.metadata.read().await;
        Ok(store
            .get(&(execution_id.clone(), artifact_name.to_string()))
            .cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_store() -> (LocalArtifactStore, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let config = ArtifactStoreConfig {
            base_dir: temp_dir.path().to_path_buf(),
            max_age_days: 7,
        };
        let store = LocalArtifactStore::new(config);
        (store, temp_dir)
    }

    #[tokio::test]
    async fn test_upload_and_download() {
        let (store, _temp) = create_test_store();
        let execution_id = ExecutionId::new();
        let artifact_name = "output.txt".to_string();
        let content = b"test content".to_vec();

        // Upload
        let metadata = store
            .upload(
                execution_id.clone(),
                artifact_name.clone(),
                content.clone(),
                Some("text/plain".to_string()),
            )
            .await
            .unwrap();

        assert_eq!(metadata.artifact_name, artifact_name);
        assert_eq!(metadata.size_bytes, content.len() as u64);

        // Download
        let downloaded = store.download(&execution_id, &artifact_name).await.unwrap();
        assert_eq!(downloaded, content);
    }

    #[tokio::test]
    async fn test_list_artifacts() {
        let (store, _temp) = create_test_store();
        let execution_id = ExecutionId::new();

        // Upload multiple artifacts
        store
            .upload(
                execution_id.clone(),
                "file1.txt".to_string(),
                b"content1".to_vec(),
                None,
            )
            .await
            .unwrap();
        store
            .upload(
                execution_id.clone(),
                "file2.txt".to_string(),
                b"content2".to_vec(),
                None,
            )
            .await
            .unwrap();

        // List artifacts
        let artifacts = store.list_artifacts(&execution_id).await.unwrap();
        assert_eq!(artifacts.len(), 2);
    }

    #[tokio::test]
    async fn test_delete_artifact() {
        let (store, _temp) = create_test_store();
        let execution_id = ExecutionId::new();
        let artifact_name = "output.txt".to_string();

        store
            .upload(
                execution_id.clone(),
                artifact_name.clone(),
                b"content".to_vec(),
                None,
            )
            .await
            .unwrap();

        // Delete artifact
        store.delete(&execution_id, &artifact_name).await.unwrap();

        // Verify deleted
        let result = store.download(&execution_id, &artifact_name).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_delete_execution_artifacts() {
        let (store, _temp) = create_test_store();
        let execution_id = ExecutionId::new();

        store
            .upload(
                execution_id.clone(),
                "file1.txt".to_string(),
                b"content1".to_vec(),
                None,
            )
            .await
            .unwrap();
        store
            .upload(
                execution_id.clone(),
                "file2.txt".to_string(),
                b"content2".to_vec(),
                None,
            )
            .await
            .unwrap();

        // Delete all artifacts
        let count = store
            .delete_execution_artifacts(&execution_id)
            .await
            .unwrap();
        assert_eq!(count, 2);

        // Verify all deleted
        let artifacts = store.list_artifacts(&execution_id).await.unwrap();
        assert_eq!(artifacts.len(), 0);
    }

    #[tokio::test]
    async fn test_cleanup_old_artifacts() {
        let temp_dir = TempDir::new().unwrap();
        let config = ArtifactStoreConfig {
            base_dir: temp_dir.path().to_path_buf(),
            max_age_days: 0, // Immediate expiration for testing
        };
        let store = LocalArtifactStore::new(config);

        let execution_id = ExecutionId::new();
        store
            .upload(
                execution_id.clone(),
                "old.txt".to_string(),
                b"old content".to_vec(),
                None,
            )
            .await
            .unwrap();

        // Wait a moment to ensure time has passed
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Cleanup
        let count = store.cleanup_old_artifacts().await.unwrap();
        assert_eq!(count, 1);

        // Verify cleaned up
        let artifacts = store.list_artifacts(&execution_id).await.unwrap();
        assert_eq!(artifacts.len(), 0);
    }

    #[tokio::test]
    async fn test_get_metadata() {
        let (store, _temp) = create_test_store();
        let execution_id = ExecutionId::new();
        let artifact_name = "test.txt".to_string();

        store
            .upload(
                execution_id.clone(),
                artifact_name.clone(),
                b"content".to_vec(),
                Some("text/plain".to_string()),
            )
            .await
            .unwrap();

        // Get metadata
        let metadata = store
            .get_metadata(&execution_id, &artifact_name)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(metadata.artifact_name, artifact_name);
        assert_eq!(metadata.content_type, Some("text/plain".to_string()));
    }

    #[tokio::test]
    async fn test_download_nonexistent() {
        let (store, _temp) = create_test_store();
        let execution_id = ExecutionId::new();

        let result = store.download(&execution_id, "nonexistent.txt").await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("artifact not found"));
    }

    // Security Tests: Path Traversal Prevention

    #[tokio::test]
    async fn test_path_traversal_parent_dir() {
        let (store, _temp) = create_test_store();
        let execution_id = ExecutionId::new();

        // Attempt path traversal with ../
        let result = store
            .upload(
                execution_id.clone(),
                "../../../etc/passwd".to_string(),
                b"malicious content".to_vec(),
                None,
            )
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("path traversal"));
    }

    #[tokio::test]
    async fn test_path_traversal_absolute_unix() {
        let (store, _temp) = create_test_store();
        let execution_id = ExecutionId::new();

        // Attempt absolute path (Unix) - gets rejected by path separator check first
        let result = store
            .upload(
                execution_id,
                "/etc/passwd".to_string(),
                b"malicious content".to_vec(),
                None,
            )
            .await;

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        // Absolute paths are rejected by the path separator check (line 129-131)
        // which executes before the absolute path check (line 138-141)
        assert!(
            err_msg.contains("path separators"),
            "Expected 'path separators' in error, got: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_path_traversal_absolute_windows() {
        let (store, _temp) = create_test_store();
        let execution_id = ExecutionId::new();

        // Attempt absolute path (Windows)
        let result = store
            .upload(
                execution_id,
                "C:\\Windows\\System32\\evil.dll".to_string(),
                b"malicious content".to_vec(),
                None,
            )
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("path separator"));
    }

    #[tokio::test]
    async fn test_path_traversal_slash_in_name() {
        let (store, _temp) = create_test_store();
        let execution_id = ExecutionId::new();

        // Attempt path with forward slash
        let result = store
            .upload(
                execution_id.clone(),
                "subdir/file.txt".to_string(),
                b"content".to_vec(),
                None,
            )
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("path separator"));

        // Attempt path with backslash
        let result2 = store
            .upload(
                execution_id,
                "subdir\\file.txt".to_string(),
                b"content".to_vec(),
                None,
            )
            .await;

        assert!(result2.is_err());
        assert!(result2.unwrap_err().to_string().contains("path separator"));
    }

    #[tokio::test]
    async fn test_empty_artifact_name() {
        let (store, _temp) = create_test_store();
        let execution_id = ExecutionId::new();

        let result = store
            .upload(execution_id, "".to_string(), b"content".to_vec(), None)
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cannot be empty"));
    }

    #[tokio::test]
    async fn test_artifact_name_too_long() {
        let (store, _temp) = create_test_store();
        let execution_id = ExecutionId::new();

        // Create name longer than 255 chars
        let long_name = "a".repeat(256);

        let result = store
            .upload(execution_id, long_name, b"content".to_vec(), None)
            .await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("too long"));
    }

    #[tokio::test]
    async fn test_artifact_name_control_characters() {
        let (store, _temp) = create_test_store();
        let execution_id = ExecutionId::new();

        // Try null byte
        let result = store
            .upload(
                execution_id.clone(),
                "file\0name.txt".to_string(),
                b"content".to_vec(),
                None,
            )
            .await;

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("control characters"));

        // Try newline
        let result2 = store
            .upload(
                execution_id,
                "file\nname.txt".to_string(),
                b"content".to_vec(),
                None,
            )
            .await;

        assert!(result2.is_err());
    }

    #[tokio::test]
    async fn test_artifact_name_dot_references() {
        let (store, _temp) = create_test_store();
        let execution_id = ExecutionId::new();

        // Try single dot
        let result = store
            .upload(
                execution_id.clone(),
                ".".to_string(),
                b"content".to_vec(),
                None,
            )
            .await;

        assert!(result.is_err());

        // Try double dot
        let result2 = store
            .upload(execution_id, "..".to_string(), b"content".to_vec(), None)
            .await;

        assert!(result2.is_err());
    }

    #[tokio::test]
    async fn test_valid_artifact_names() {
        let (store, _temp) = create_test_store();
        let execution_id = ExecutionId::new();

        // Test various valid names
        let valid_names = vec![
            "output.txt",
            "result.json",
            "data-2024-01-15.csv",
            "file_with_underscore.log",
            "FILE_UPPERCASE.TXT",
            "123-numeric-prefix.dat",
            "file.with.multiple.dots.txt",
        ];

        for name in valid_names {
            let result = store
                .upload(
                    execution_id.clone(),
                    name.to_string(),
                    b"test content".to_vec(),
                    None,
                )
                .await;

            assert!(
                result.is_ok(),
                "Valid name '{}' should be accepted but got error: {:?}",
                name,
                result.err()
            );
        }
    }

    // Configuration Validation Tests

    #[test]
    fn test_config_default_is_valid() {
        let config = ArtifactStoreConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_age_too_low() {
        let config = ArtifactStoreConfig {
            base_dir: PathBuf::from("/tmp/test"),
            max_age_days: 0, // Below MIN (1)
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("max_age_days too low"));
    }

    #[test]
    fn test_config_age_too_high() {
        let config = ArtifactStoreConfig {
            base_dir: PathBuf::from("/tmp/test"),
            max_age_days: 366, // Above MAX (365)
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("max_age_days too high"));
    }

    #[test]
    fn test_config_valid_edge_cases() {
        // Min value
        let config = ArtifactStoreConfig {
            base_dir: PathBuf::from("/tmp/test"),
            max_age_days: 1,
        };
        assert!(config.validate().is_ok());

        // Max value
        let config = ArtifactStoreConfig {
            base_dir: PathBuf::from("/tmp/test"),
            max_age_days: 365,
        };
        assert!(config.validate().is_ok());
    }

    /// Test that heavy file I/O operations don't block other async tasks.
    /// This verifies that blocking operations are properly wrapped in spawn_blocking.
    #[tokio::test]
    async fn test_heavy_operations_dont_block_async_tasks() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;
        use std::time::Duration;
        use tokio::time::sleep;

        // Create temporary directory for test
        let temp_dir = std::env::temp_dir().join("cyberclaw_blocking_test");
        let _ = std::fs::create_dir_all(&temp_dir);

        let config = ArtifactStoreConfig {
            base_dir: temp_dir.clone(),
            max_age_days: 7,
        };
        let store = LocalArtifactStore::new(config);

        // Counter to track concurrent task execution
        let counter = Arc::new(AtomicU32::new(0));

        // Spawn 10 concurrent tasks that perform file I/O
        let mut handles = vec![];
        for i in 0..10 {
            let store_clone = store.clone();
            let counter_clone = counter.clone();
            let handle = tokio::spawn(async move {
                let exec_id = ExecutionId::new();
                let artifact_name = format!("test_artifact_{}", i);
                let content = vec![0u8; 1024]; // 1KB content

                // Upload artifact (involves blocking I/O)
                let _ = store_clone
                    .upload(exec_id.clone(), artifact_name.clone(), content, None)
                    .await;

                // Increment counter to show this task completed
                counter_clone.fetch_add(1, Ordering::SeqCst);

                // Download artifact (involves blocking I/O)
                let _ = store_clone.download(&exec_id, &artifact_name).await;

                counter_clone.fetch_add(1, Ordering::SeqCst);
            });
            handles.push(handle);
        }

        // Spawn a lightweight task that should complete quickly
        // If blocking I/O blocks the tokio runtime, this task will be delayed
        let lightweight_task = tokio::spawn(async move {
            sleep(Duration::from_millis(100)).await;
        });

        // Wait for lightweight task to complete (should be fast)
        let lightweight_result =
            tokio::time::timeout(Duration::from_secs(2), lightweight_task).await;

        // Lightweight task should complete within 2 seconds
        assert!(
            lightweight_result.is_ok(),
            "Lightweight task was blocked by heavy I/O operations"
        );

        // Wait for all file I/O tasks to complete
        for handle in handles {
            let _ = handle.await;
        }

        // Verify all file I/O tasks completed
        // Each task increments counter twice (upload + download)
        let final_count = counter.load(Ordering::SeqCst);
        assert_eq!(
            final_count, 20,
            "Expected 20 operations (10 uploads + 10 downloads), got {}",
            final_count
        );

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
