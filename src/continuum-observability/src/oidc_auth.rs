use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcConfig {
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcToken {
    pub access_token: String,
    pub id_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub claims: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LdapConfig {
    pub url: String,
    pub bind_dn: String,
    pub bind_password: String,
    pub base_dn: String,
    pub filter: String,
    pub attribute_map: HashMap<String, String>,
}

pub struct IdentityProvider {
    oidc_config: Option<OidcConfig>,
    ldap_config: Option<LdapConfig>,
}

impl IdentityProvider {
    pub fn new() -> Self {
        Self {
            oidc_config: None,
            ldap_config: None,
        }
    }

    pub fn configure_oidc(&mut self, config: OidcConfig) {
        self.oidc_config = Some(config);
        tracing::info!("OIDC configured");
    }

    pub fn configure_ldap(&mut self, config: LdapConfig) {
        self.ldap_config = Some(config);
        tracing::info!("LDAP configured");
    }

    #[cfg(feature = "oidc")]
    pub async fn authenticate_oidc(&self, code: &str) -> Result<OidcToken> {
        let config = self
            .oidc_config
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("OIDC not configured"))?;

        let token_url = format!("{}/token", config.issuer_url.trim_end_matches('/'));

        let client = reqwest::Client::new();
        let params = [
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", &config.redirect_uri),
            ("client_id", &config.client_id),
            ("client_secret", &config.client_secret),
        ];

        let resp = client
            .post(&token_url)
            .form(&params)
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?;

        let access_token = resp["access_token"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing access_token"))?
            .to_string();
        let id_token = resp["id_token"].as_str().unwrap_or("").to_string();
        let expires_in = resp["expires_in"].as_u64().unwrap_or(3600);

        let claims: HashMap<String, serde_json::Value> =
            serde_json::from_str(&decode_jwt_payload(&id_token)).unwrap_or_default();

        let token = OidcToken {
            access_token,
            id_token,
            refresh_token: resp["refresh_token"].as_str().map(|s| s.to_string()),
            expires_at: chrono::Utc::now() + chrono::Duration::seconds(expires_in as i64),
            claims,
        };

        Ok(token)
    }

    #[cfg(not(feature = "oidc"))]
    pub async fn authenticate_oidc(&self, _code: &str) -> Result<OidcToken> {
        Err(anyhow::anyhow!(
            "OIDC support not enabled. Build with --features oidc"
        ))
    }

    pub async fn authenticate_ldap(
        &self,
        _username: &str,
        _password: &str,
    ) -> Result<HashMap<String, String>> {
        Err(anyhow::anyhow!("LDAP authentication not yet implemented"))
    }

    pub fn is_configured(&self) -> bool {
        self.oidc_config.is_some() || self.ldap_config.is_some()
    }
}

impl Default for IdentityProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(dead_code)]
fn decode_jwt_payload(token: &str) -> String {
    if let Some(payload) = token.split('.').nth(1) {
        use base64::Engine;
        if let Ok(decoded) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload) {
            return String::from_utf8_lossy(&decoded).to_string();
        }
    }
    "{}".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identity_provider_default() {
        let idp = IdentityProvider::new();
        assert!(!idp.is_configured());
    }

    #[test]
    fn test_identity_provider_configure_oidc() {
        let mut idp = IdentityProvider::new();
        idp.configure_oidc(OidcConfig {
            issuer_url: "https://accounts.google.com".to_string(),
            client_id: "test".to_string(),
            client_secret: "secret".to_string(),
            redirect_uri: "http://localhost".to_string(),
            scopes: vec!["openid".to_string()],
        });
        assert!(idp.is_configured());
    }

    #[test]
    fn test_identity_provider_configure_ldap() {
        let mut idp = IdentityProvider::new();
        idp.configure_ldap(LdapConfig {
            url: "ldap://localhost".to_string(),
            bind_dn: "cn=admin".to_string(),
            bind_password: "pass".to_string(),
            base_dn: "dc=example,dc=com".to_string(),
            filter: "(uid={})".to_string(),
            attribute_map: std::collections::HashMap::new(),
        });
        assert!(idp.is_configured());
    }

    #[test]
    fn test_oidc_config_serialization() {
        let config = OidcConfig {
            issuer_url: "https://example.com".to_string(),
            client_id: "client".to_string(),
            client_secret: "secret".to_string(),
            redirect_uri: "http://localhost/callback".to_string(),
            scopes: vec!["openid".to_string(), "profile".to_string()],
        };
        let json = serde_json::to_string(&config).unwrap();
        let decoded: OidcConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.issuer_url, "https://example.com");
        assert_eq!(decoded.scopes.len(), 2);
    }

    #[test]
    fn test_oidc_token_serialization() {
        let token = OidcToken {
            access_token: "at_123".to_string(),
            id_token: "id_456".to_string(),
            refresh_token: Some("rt_789".to_string()),
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            claims: std::collections::HashMap::new(),
        };
        let json = serde_json::to_string(&token).unwrap();
        let decoded: OidcToken = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.access_token, "at_123");
        assert!(decoded.refresh_token.is_some());
    }

    #[test]
    fn test_ldap_config_serialization() {
        let config = LdapConfig {
            url: "ldap://dc.example.com".to_string(),
            bind_dn: "cn=service".to_string(),
            bind_password: "password".to_string(),
            base_dn: "dc=example,dc=com".to_string(),
            filter: "(sAMAccountName={})".to_string(),
            attribute_map: {
                let mut m = std::collections::HashMap::new();
                m.insert("email".to_string(), "mail".to_string());
                m
            },
        };
        let json = serde_json::to_string(&config).unwrap();
        let decoded: LdapConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.url, "ldap://dc.example.com");
    }

    #[test]
    fn test_decode_jwt_payload_invalid() {
        let result = decode_jwt_payload("not-a-jwt");
        assert_eq!(result, "{}");
    }

    #[test]
    fn test_decode_jwt_payload_empty() {
        let result = decode_jwt_payload("");
        assert_eq!(result, "{}");
    }
}
