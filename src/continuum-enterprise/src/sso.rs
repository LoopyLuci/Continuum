use serde::{Deserialize, Serialize};

/// SSO configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SsoConfig {
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub redirect_url: String,
    pub scopes: Vec<String>,
}

/// SSO provider trait
pub trait SsoProvider: Send + Sync {
    /// Get the authorization URL
    fn authorize_url(&self, state: &str) -> String;

    /// Exchange authorization code for tokens
    fn exchange_code(&self, code: &str) -> Result<TokenPair, SsoError>;

    /// Get user info from access token
    fn user_info(&self, access_token: &str) -> Result<UserInfo, SsoError>;
}

/// Token pair
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
}

/// User info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub id: String,
    pub email: String,
    pub name: String,
    pub groups: Vec<String>,
}

/// SSO errors
#[derive(Debug, thiserror::Error)]
pub enum SsoError {
    #[error("Authentication failed: {0}")]
    AuthFailed(String),
    #[error("Token exchange failed: {0}")]
    TokenExchangeFailed(String),
    #[error("User info fetch failed: {0}")]
    UserInfoFailed(String),
    #[error("Configuration error: {0}")]
    ConfigError(String),
}

/// OIDC provider placeholder
pub struct OidcProvider {
    config: SsoConfig,
}

impl OidcProvider {
    pub fn new(config: SsoConfig) -> Self {
        Self { config }
    }
}

impl SsoProvider for OidcProvider {
    fn authorize_url(&self, state: &str) -> String {
        format!(
            "{}/authorize?client_id={}&redirect_uri={}&response_type=code&scope={}&state={}",
            self.config.issuer_url,
            self.config.client_id,
            self.config.redirect_url,
            self.config.scopes.join(" "),
            state
        )
    }

    fn exchange_code(&self, _code: &str) -> Result<TokenPair, SsoError> {
        Err(SsoError::AuthFailed("Not implemented".into()))
    }

    fn user_info(&self, _access_token: &str) -> Result<UserInfo, SsoError> {
        Err(SsoError::UserInfoFailed("Not implemented".into()))
    }
}

/// OAuth2 provider placeholder
pub struct OAuth2Provider {
    config: SsoConfig,
}

impl OAuth2Provider {
    pub fn new(config: SsoConfig) -> Self {
        Self { config }
    }
}

impl SsoProvider for OAuth2Provider {
    fn authorize_url(&self, state: &str) -> String {
        format!(
            "{}/oauth/authorize?client_id={}&redirect_uri={}&response_type=code&scope={}&state={}",
            self.config.issuer_url,
            self.config.client_id,
            self.config.redirect_url,
            self.config.scopes.join(" "),
            state
        )
    }

    fn exchange_code(&self, _code: &str) -> Result<TokenPair, SsoError> {
        Err(SsoError::AuthFailed("Not implemented".into()))
    }

    fn user_info(&self, _access_token: &str) -> Result<UserInfo, SsoError> {
        Err(SsoError::UserInfoFailed("Not implemented".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_oidc_authorize_url() {
        let config = SsoConfig {
            issuer_url: "https://auth.example.com".into(),
            client_id: "test-client".into(),
            client_secret: "secret".into(),
            redirect_url: "https://app.example.com/callback".into(),
            scopes: vec!["openid".into(), "profile".into()],
        };
        let provider = OidcProvider::new(config);
        let url = provider.authorize_url("random-state");
        assert!(url.contains("client_id=test-client"));
        assert!(url.contains("state=random-state"));
    }
}
