use serde::{Deserialize, Serialize};

/// 终端引导提示流程的遥测事件。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum OnboardingEvent {
    /// A callout was displayed.
    CalloutDisplayed { callout: String },
    /// The user clicked next on a callout.
    CalloutNext,
    /// The user completed the callout flow.
    CalloutCompleted { completion_type: String },
}
