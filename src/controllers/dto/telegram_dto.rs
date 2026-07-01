use serde::Deserialize;

#[derive(Deserialize)]
pub struct ConfigureTelegramDTO {
    pub(crate) bot_token: String,
    pub(crate) chat_id: String,
    pub(crate) etherscan_api_key: Option<String>,
    pub(crate) bot_username: Option<String>,
}

#[derive(Deserialize)]
pub struct TestTelegramDTO {
    pub(crate) message: Option<String>,
}
