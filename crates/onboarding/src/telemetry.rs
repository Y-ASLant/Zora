use serde::{Deserialize, Serialize};

/// Telemetry events for the onboarding flow.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum OnboardingEvent {
    /// A callout was displayed.
    CalloutDisplayed { callout: String },
    /// The user clicked next on a callout.
    CalloutNext,
    /// The user completed the callout flow.
    CalloutCompleted { completion_type: String },
    /// The user navigated to the next slide.
    SlideNavigatedNext,
    /// The user navigated to the previous slide.
    SlideNavigatedBack,
}
