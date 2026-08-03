// Onboarding library crate

pub mod callout;
mod localization;
pub mod telemetry;

/// 引导提示中用户选择的使用意图。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnboardingIntention {
    Terminal,
    AgentDrivenDevelopment,
}

impl std::fmt::Display for OnboardingIntention {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OnboardingIntention::AgentDrivenDevelopment => write!(f, "agent_driven"),
            OnboardingIntention::Terminal => write!(f, "terminal"),
        }
    }
}

pub use callout::{OnboardingCalloutView, OnboardingKeybindings};
pub use localization::set_localizer;

pub mod components;

pub use telemetry::OnboardingEvent;

pub fn init(app: &mut warpui::AppContext) {
    callout::init(app);
}
