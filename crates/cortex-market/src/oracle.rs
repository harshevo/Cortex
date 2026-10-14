//! The app's model provider as an [`Oracle`]: the language-model forecast
//! asks the same model, with the same sign-in or key, as the agent.

use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use cortex_core::{Image, Message, ModelProvider, ModelRequest};

use crate::llm::{Answer, Oracle, Picture};

const SYSTEM: &str = "You are a careful market analyst. You answer with JSON only.";

pub struct ProviderOracle {
    provider: Arc<dyn ModelProvider>,
}

impl ProviderOracle {
    pub fn new(provider: Arc<dyn ModelProvider>) -> Self {
        Self { provider }
    }
}

impl Oracle for ProviderOracle {
    fn name(&self) -> String {
        self.provider.model().to_string()
    }

    fn ask(&self, prompt: String, picture: Option<Picture>) -> Answer {
        let provider = self.provider.clone();

        Box::pin(async move {
            let images: Vec<Image> = picture
                .into_iter()
                .map(|picture| Image {
                    media_type: picture.media_type,
                    data: STANDARD.encode(&picture.bytes),
                })
                .collect();

            let messages = [Message::user_with_images(prompt, &images)];

            let response = provider
                .complete(ModelRequest {
                    system: SYSTEM,
                    messages: &messages,
                    tools: &[],
                })
                .await
                .map_err(|error| error.to_string())?;

            Ok(response.message.text())
        })
    }
}
