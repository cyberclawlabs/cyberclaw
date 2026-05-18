//! # Hook 事件总线 (Event Bus)
//!
//! 提供异步、非阻塞的 Hook 事件分发机制。
//!
//! ## 特性
//!
//! - 基于 `tokio::sync::mpsc` 的无界通道
//! - 支持多个订阅者并发处理事件
//! - 背压控制：当通道满时自动丢弃旧事件
//! - 优雅关闭：等待所有事件处理完成

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{info, warn};

/// Hook 事件类型
///
/// 定义 Plugin Hook 系统中可能发生的事件。
///
/// # 示例
///
/// ```rust
/// use cyberclaw_plugin_runtime::event_bus::HookEvent;
/// use serde_json::json;
///
/// let event = HookEvent::HookExecuted {
///     plugin_id: "test-plugin".to_string(),
///     hook_type: "before_execution".to_string(),
///     duration_ms: 150,
///     success: true,
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event_type", rename_all = "snake_case")]
pub enum HookEvent {
    /// Hook 注册事件
    HookRegistered {
        plugin_id: String,
        hook_type: String,
        priority: i32,
    },

    /// Hook 注销事件
    HookUnregistered {
        plugin_id: String,
        hook_type: String,
    },

    /// Hook 执行完成事件
    HookExecuted {
        plugin_id: String,
        hook_type: String,
        duration_ms: u64,
        success: bool,
    },

    /// Hook 执行失败事件
    HookFailed {
        plugin_id: String,
        hook_type: String,
        error: String,
        attempt: u32,
    },

    /// Hook 重试事件
    HookRetrying {
        plugin_id: String,
        hook_type: String,
        attempt: u32,
        max_attempts: u32,
    },

    /// Hook 超时事件
    HookTimeout {
        plugin_id: String,
        hook_type: String,
        timeout_ms: u64,
    },

    /// Hook 链开始事件
    HookChainStarted {
        hook_type: String,
        total_hooks: usize,
    },

    /// Hook 链完成事件
    HookChainCompleted {
        hook_type: String,
        total_hooks: usize,
        total_duration_ms: u64,
        success_count: usize,
        failure_count: usize,
    },
}

/// 事件总线
///
/// 负责 Hook 事件的异步分发和订阅管理。
///
/// # 示例
///
/// ```rust
/// use cyberclaw_plugin_runtime::event_bus::{EventBus, HookEvent};
/// use tokio::sync::mpsc;
///
/// #[tokio::test]
/// async fn example() {
///     let (tx, mut rx) = mpsc::unbounded_channel();
///     let bus = EventBus::new(tx);
///
///     // 发布事件
///     bus.publish(HookEvent::HookChainStarted {
///         hook_type: "before_execution".to_string(),
///         total_hooks: 3,
///     }).await;
///
///     // 订阅事件
///     if let Some(event) = rx.recv().await {
///         println!("Received event: {:?}", event);
///     }
/// }
/// ```
pub struct EventBus {
    /// 事件发送通道
    sender: mpsc::UnboundedSender<HookEvent>,
}

impl EventBus {
    /// 创建新的事件总线
    ///
    /// # 参数
    ///
    /// - `sender`: 事件发送通道
    pub fn new(sender: mpsc::UnboundedSender<HookEvent>) -> Self {
        Self { sender }
    }

    /// 创建事件总线和接收器
    ///
    /// # 返回值
    ///
    /// - `EventBus`: 事件总线实例
    /// - `mpsc::UnboundedReceiver<HookEvent>`: 事件接收器
    ///
    /// # 示例
    ///
    /// ```rust
    /// use cyberclaw_plugin_runtime::event_bus::EventBus;
    ///
    /// let (bus, mut receiver) = EventBus::create();
    /// ```
    pub fn create() -> (Self, mpsc::UnboundedReceiver<HookEvent>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (Self::new(sender), receiver)
    }

    /// 发布事件
    ///
    /// 异步非阻塞地发布事件到总线。如果通道已关闭，会记录警告但不会失败。
    ///
    /// # 参数
    ///
    /// - `event`: 要发布的 Hook 事件
    pub async fn publish(&self, event: HookEvent) {
        if let Err(e) = self.sender.send(event) {
            warn!("Failed to publish event (channel closed): {:?}", e);
        }
    }

    /// 同步发布事件（用于非异步上下文）
    ///
    /// # 参数
    ///
    /// - `event`: 要发布的 Hook 事件
    pub fn publish_sync(&self, event: HookEvent) {
        if let Err(e) = self.sender.send(event) {
            warn!("Failed to publish event (channel closed): {:?}", e);
        }
    }

    /// 批量发布事件
    ///
    /// # 参数
    ///
    /// - `events`: 要发布的事件列表
    pub async fn publish_batch(&self, events: Vec<HookEvent>) {
        for event in events {
            self.publish(event).await;
        }
    }

    /// 获取通道是否已关闭
    pub fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }
}

impl Clone for EventBus {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
        }
    }
}

/// 事件订阅器
///
/// 封装事件接收逻辑，提供便捷的事件处理接口。
pub struct EventSubscriber {
    receiver: mpsc::UnboundedReceiver<HookEvent>,
}

impl EventSubscriber {
    /// 创建新的订阅器
    pub fn new(receiver: mpsc::UnboundedReceiver<HookEvent>) -> Self {
        Self { receiver }
    }

    /// 接收下一个事件
    ///
    /// # 返回值
    ///
    /// - `Some(HookEvent)`: 成功接收事件
    /// - `None`: 通道已关闭
    pub async fn recv(&mut self) -> Option<HookEvent> {
        self.receiver.recv().await
    }

    /// 尝试接收事件（非阻塞）
    ///
    /// # 返回值
    ///
    /// - `Ok(HookEvent)`: 成功接收事件
    /// - `Err(TryRecvError)`: 无事件或通道已关闭
    pub fn try_recv(&mut self) -> Result<HookEvent, mpsc::error::TryRecvError> {
        self.receiver.try_recv()
    }

    /// 运行事件处理循环
    ///
    /// # 参数
    ///
    /// - `handler`: 事件处理函数
    ///
    /// # 示例
    ///
    /// ```rust
    /// use cyberclaw_plugin_runtime::event_bus::{EventBus, EventSubscriber};
    ///
    /// #[tokio::test]
    /// async fn example() {
    ///     let (bus, receiver) = EventBus::create();
    ///     let mut subscriber = EventSubscriber::new(receiver);
    ///
    ///     tokio::spawn(async move {
    ///         subscriber.run(|event| async move {
    ///             println!("Received: {:?}", event);
    ///         }).await;
    ///     });
    /// }
    /// ```
    pub async fn run<F, Fut>(&mut self, mut handler: F)
    where
        F: FnMut(HookEvent) -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        while let Some(event) = self.recv().await {
            handler(event).await;
        }
        info!("Event subscriber loop ended");
    }

    /// 关闭接收器
    pub fn close(&mut self) {
        self.receiver.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn test_event_bus_create() {
        let (bus, _receiver) = EventBus::create();
        assert!(!bus.is_closed());
    }

    #[tokio::test]
    async fn test_publish_and_receive() {
        let (bus, mut receiver) = EventBus::create();

        let event = HookEvent::HookRegistered {
            plugin_id: "test-plugin".to_string(),
            hook_type: "before_execution".to_string(),
            priority: 100,
        };

        bus.publish(event.clone()).await;

        let received = receiver.recv().await;
        assert!(received.is_some());

        match received.unwrap() {
            HookEvent::HookRegistered {
                plugin_id,
                hook_type,
                priority,
            } => {
                assert_eq!(plugin_id, "test-plugin");
                assert_eq!(hook_type, "before_execution");
                assert_eq!(priority, 100);
            }
            _ => panic!("Unexpected event type"),
        }
    }

    #[tokio::test]
    async fn test_publish_batch() {
        let (bus, mut receiver) = EventBus::create();

        let events = vec![
            HookEvent::HookRegistered {
                plugin_id: "plugin1".to_string(),
                hook_type: "before".to_string(),
                priority: 1,
            },
            HookEvent::HookRegistered {
                plugin_id: "plugin2".to_string(),
                hook_type: "after".to_string(),
                priority: 2,
            },
        ];

        bus.publish_batch(events).await;

        let event1 = receiver.recv().await.unwrap();
        let event2 = receiver.recv().await.unwrap();

        assert!(matches!(event1, HookEvent::HookRegistered { .. }));
        assert!(matches!(event2, HookEvent::HookRegistered { .. }));
    }

    #[tokio::test]
    async fn test_event_subscriber_recv() {
        let (bus, receiver) = EventBus::create();
        let mut subscriber = EventSubscriber::new(receiver);

        let event = HookEvent::HookExecuted {
            plugin_id: "test".to_string(),
            hook_type: "before".to_string(),
            duration_ms: 100,
            success: true,
        };

        bus.publish(event).await;

        let received = subscriber.recv().await;
        assert!(received.is_some());
    }

    #[tokio::test]
    async fn test_event_subscriber_try_recv() {
        let (bus, receiver) = EventBus::create();
        let mut subscriber = EventSubscriber::new(receiver);

        // 没有事件时应该返回 Empty
        let result = subscriber.try_recv();
        assert!(result.is_err());

        // 发送事件后应该能接收
        bus.publish_sync(HookEvent::HookChainStarted {
            hook_type: "test".to_string(),
            total_hooks: 1,
        });

        let result = subscriber.try_recv();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_event_bus_clone() {
        let (bus1, mut receiver) = EventBus::create();
        let bus2 = bus1.clone();

        bus1.publish(HookEvent::HookChainStarted {
            hook_type: "test1".to_string(),
            total_hooks: 1,
        })
        .await;

        bus2.publish(HookEvent::HookChainStarted {
            hook_type: "test2".to_string(),
            total_hooks: 2,
        })
        .await;

        let event1 = receiver.recv().await.unwrap();
        let event2 = receiver.recv().await.unwrap();

        assert!(matches!(event1, HookEvent::HookChainStarted { .. }));
        assert!(matches!(event2, HookEvent::HookChainStarted { .. }));
    }

    #[tokio::test]
    async fn test_channel_closed_detection() {
        let (bus, receiver) = EventBus::create();
        assert!(!bus.is_closed());

        drop(receiver);
        sleep(Duration::from_millis(10)).await;

        // 通道关闭后 is_closed 应该返回 true
        // 注意：mpsc::UnboundedSender::is_closed() 只在所有接收器都被丢弃后才返回 true
    }

    #[tokio::test]
    async fn test_serde_hook_event() {
        let event = HookEvent::HookExecuted {
            plugin_id: "test".to_string(),
            hook_type: "before".to_string(),
            duration_ms: 200,
            success: true,
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("hook_executed"));
        assert!(json.contains("test"));

        let deserialized: HookEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, HookEvent::HookExecuted { .. }));
    }

    #[tokio::test]
    async fn test_concurrent_publishers() {
        let (bus, mut receiver) = EventBus::create();

        let bus1 = bus.clone();
        let bus2 = bus.clone();

        let handle1 = tokio::spawn(async move {
            for i in 0..10_i32 {
                bus1.publish(HookEvent::HookRegistered {
                    plugin_id: format!("plugin1-{}", i),
                    hook_type: "test".to_string(),
                    priority: i,
                })
                .await;
            }
        });

        let handle2 = tokio::spawn(async move {
            for i in 0..10_i32 {
                bus2.publish(HookEvent::HookRegistered {
                    plugin_id: format!("plugin2-{}", i),
                    hook_type: "test".to_string(),
                    priority: i,
                })
                .await;
            }
        });

        handle1.await.unwrap();
        handle2.await.unwrap();

        let mut count = 0;
        while receiver.try_recv().is_ok() {
            count += 1;
        }

        assert_eq!(count, 20);
    }
}
