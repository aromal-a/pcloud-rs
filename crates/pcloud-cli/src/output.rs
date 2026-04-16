// **PLATFORM:** all
// **GATING:** none (portable).

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedOutput {
    pub title: String,
}

impl RenderedOutput {
    #[must_use]
    pub fn from_message(message: impl Into<String>) -> Self {
        Self {
            title: message.into(),
        }
    }
}
