use crate::{Error, Result};
use serde::Deserialize;
use url::Url;

// Deliberately no Debug: both long-lived and asset-scoped tokens are credentials.
pub(crate) struct Ion {
    pub endpoint: Url,
    pub token: String,
    pub grant: Option<Grant>,
}
pub(crate) struct Grant {
    pub url: Url,
    pub token: String,
    pub credits: Vec<String>,
}
impl Ion {
    pub fn new(asset: i64, token: &str) -> Result<Self> {
        if asset <= 0 || token.trim().is_empty() || token.contains(['\r', '\n']) {
            return Err(Error("Invalid ion asset or token"));
        }
        Ok(Self {
            endpoint: Url::parse(&format!(
                "https://api.cesium.com/v1/assets/{asset}/endpoint"
            ))
            .map_err(|_| Error("Invalid ion endpoint"))?,
            token: token.into(),
            grant: None,
        })
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Endpoint {
    #[serde(default)]
    url: String,
    #[serde(default)]
    access_token: String,
    #[serde(rename = "type")]
    #[serde(default)]
    kind: String,
    external_type: Option<String>,
    options: Option<ExternalOptions>,
    #[serde(default)]
    attributions: Vec<Attribution>,
}
#[derive(Deserialize)]
struct ExternalOptions {
    url: String,
}
#[derive(Deserialize)]
struct Attribution {
    html: String,
}
impl Grant {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let e: Endpoint =
            serde_json::from_slice(bytes).map_err(|_| Error("Invalid ion endpoint response"))?;
        let external = e.external_type.as_deref();
        let kind = external.unwrap_or(e.kind.as_str());
        if !matches!(kind, "3DTILES" | "TERRAIN") || external.is_some_and(|t| t != "3DTILES") {
            return Err(Error("Unsupported ion asset type or external provider"));
        }
        let endpoint_url = if external.is_some() {
            e.options.map(|o| o.url).unwrap_or(e.url)
        } else {
            e.url
        };
        let mut url = Url::parse(&endpoint_url).map_err(|_| Error("Invalid ion asset URL"))?;
        if kind == "TERRAIN" {
            url = url
                .join("layer.json")
                .map_err(|_| Error("Invalid terrain endpoint"))?;
        }
        if url.scheme() != "https" && !(cfg!(test) && url.scheme() == "http")
            || !url.username().is_empty()
            || url.password().is_some()
            || (external.is_none() && e.access_token.is_empty())
            || e.access_token.contains(['\r', '\n'])
        {
            return Err(Error("Invalid ion asset URL or access token"));
        }
        Ok(Self {
            url,
            token: e.access_token,
            credits: e.attributions.into_iter().map(|a| a.html).collect(),
        })
    }
    pub fn authorizes(&self, url: &Url) -> bool {
        if self.token.is_empty() || crate::google_url(&self.url) {
            return false;
        }
        let directory = self.url.path().rsplit_once('/').map_or("/", |(p, _)| p);
        url.origin() == self.url.origin()
            && (url == &self.url || url.path().starts_with(&format!("{directory}/")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn external_3dtiles_url_does_not_receive_ion_bearer() {
        let g=Grant::parse(br#"{"type":"3DTILES","externalType":"3DTILES","options":{"url":"https://tile.googleapis.com/v1/3dtiles/root.json?key=secret"},"attributions":[]}"#).unwrap();
        assert!(crate::google_url(&g.url));
        assert!(!g.authorizes(&g.url));
        assert!(g.token.is_empty());
        assert!(Grant::parse(br#"{"externalType":"3DTILES","options":{"url":"https://tile.googleapis.com/v1/3dtiles/root.json?key=secret"}}"#).is_ok());
    }
    #[test]
    fn credentials_are_scoped_to_the_asset_directory() {
        let g = Grant::parse(br#"{"type":"3DTILES","url":"https://assets.example/42/tileset.json","accessToken":"secret","attributions":[{"html":"credit"}]}"#).unwrap();
        assert!(g.authorizes(&Url::parse("https://assets.example/42/a.glb").unwrap()));
        for url in [
            "https://other.example/42/a.glb",
            "https://assets.example/43/a.glb",
            "http://assets.example/42/a.glb",
            "https://assets.example/42/../43/a.glb",
        ] {
            assert!(!g.authorizes(&Url::parse(url).unwrap()));
        }
        assert_eq!(g.credits, ["credit"]);
    }
    #[test]
    fn unsupported_provider_and_invalid_credentials_are_explicit() {
        assert!(Ion::new(0, "secret").is_err());
        assert!(Ion::new(1, "secret\n").is_err());
        let error = Grant::parse(br#"{"type":"3DTILES","externalType":"GOOGLE_3D_TILES","url":"https://secret.example/","accessToken":"secret"}"#).err().unwrap();
        assert!(error.0.contains("Unsupported"));
        assert!(!error.to_string().contains("secret"));
    }
}
