pub mod discord;
pub mod inbound_images;
pub mod telegram;
pub mod wecom;
pub mod wecom_aibot;
pub mod wecom_crypt;
pub mod whatsapp;

/// Immediate user-visible ack on external channels while the agent runs.
/// WeCom AI Bot uses this as the stream placeholder (`finish=false`); other
/// platforms send it as a short interim message (deleted where the API allows).
pub const CHANNEL_PROCESSING_ACK: &str = "Processing…";
