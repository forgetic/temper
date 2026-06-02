use async_trait::async_trait;

use crate::{ConversationReply, ConversationRequest, InteractionError};

/// Object-safe one-turn interface for an interactive conversation profile.
///
/// Responders receive an immutable transcript view and profile context. They
/// return a conversational reply plus optional inert proposals; they do not
/// receive Forge handles or mutate workflow state. This trait adapts in-process
/// responders; external responders use the same serializable request/reply and
/// proposal types through [`crate::ProcessResponder`]'s process boundary.
#[async_trait]
pub trait InteractiveResponder: Send + Sync {
    /// Runs one interaction turn.
    async fn respond(
        &self,
        request: &ConversationRequest,
    ) -> Result<ConversationReply, InteractionError>;
}
