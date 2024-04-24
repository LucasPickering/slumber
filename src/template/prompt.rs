use crate::util::ResultExt;
use anyhow::anyhow;
use std::fmt::Debug;
use tokio::sync::oneshot;

/// A prompter is a bridge between the user and the template engine. It enables
/// the template engine to request values from the user *during* the template
/// process. The implementor is responsible for deciding *how* to ask the user.
///
/// **Note:** The prompter has to be able to handle simultaneous prompt
/// requests, if a template has multiple prompt values, or if multiple templates
/// with prompts are being rendered simultaneously.  The implementor is
/// responsible for queueing prompts to show to the user one at a time.
pub trait Prompter: Debug + Send + Sync {
    /// Ask the user a question, and use the given channel to return a response.
    /// To indicate "no response", simply drop the returner.
    ///
    /// If an error occurs while prompting the user, just drop the returner.
    /// The implementor is responsible for logging the error as appropriate.
    fn prompt(&self, prompt: Prompt);
}

/// Data defining a prompt which should be presented to the user
#[derive(Debug)]
pub struct Prompt {
    /// Tell the user what we're asking for
    pub(super) message: String,
    /// Should the value the user is typing be masked? E.g. password input
    pub(super) sensitive: bool,
    /// How the prompter will pass the answer back
    pub(super) channel: oneshot::Sender<String>,
}

impl Prompt {
    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn sensitive(&self) -> bool {
        self.sensitive
    }

    /// Return the value that the user gave
    pub fn respond(self, response: String) {
        // This error *shouldn't* ever happen, because the templating task
        // stays open until it gets a response
        let _ = self
            .channel
            .send(response)
            .map_err(|_| anyhow!("Prompt listener dropped"))
            .traced();
    }
}
