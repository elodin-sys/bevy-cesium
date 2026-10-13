/// Source credentials are intentionally not Debug or Reflect.
pub enum CesiumSource {
    Url(String),
    Ion { asset_id: i64, access_token: String },
}

/// Invalid configuration. Values are never included: URLs and tokens may be secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CesiumConfigError(&'static str);
impl std::fmt::Display for CesiumConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for CesiumConfigError {}

impl CesiumSource {
    /// Optional runtime configuration. Ion takes precedence over the tileset URL.
    /// No source means `Ok(None)`; an incomplete ion configuration is an error.
    /// This does not contact ion or validate account permissions.
    pub fn from_env() -> Result<Option<Self>, CesiumConfigError> {
        fn read(key: &'static str) -> Result<Option<String>, CesiumConfigError> {
            match std::env::var(key) {
                Ok(value) => Ok(Some(value)),
                Err(std::env::VarError::NotPresent) => Ok(None),
                Err(_) => Err(CesiumConfigError(
                    "Cesium environment variables must be valid Unicode",
                )),
            }
        }
        Self::from_values(
            read("CESIUM_TILESET_URL")?.as_deref(),
            read("CESIUM_ION_ASSET_ID")?.as_deref(),
            read("CESIUM_ION_TOKEN")?.as_deref(),
        )
    }

    fn from_values(
        url: Option<&str>,
        asset: Option<&str>,
        token: Option<&str>,
    ) -> Result<Option<Self>, CesiumConfigError> {
        if let Some(asset) = asset {
            let asset_id = asset
                .trim()
                .parse::<i64>()
                .ok()
                .filter(|id| *id > 0)
                .ok_or(CesiumConfigError(
                    "CESIUM_ION_ASSET_ID must be a positive integer",
                ))?;
            let access_token = token
                .filter(|s| !s.trim().is_empty())
                .ok_or(CesiumConfigError("Set CESIUM_ION_TOKEN for the ion asset"))?;
            return Ok(Some(Self::Ion {
                asset_id,
                access_token: access_token.into(),
            }));
        }
        if token.is_some() {
            return Err(CesiumConfigError(
                "Set CESIUM_ION_ASSET_ID or unset CESIUM_ION_TOKEN",
            ));
        }
        match url {
            Some(url) if url.trim().is_empty() => {
                Err(CesiumConfigError("CESIUM_TILESET_URL must not be empty"))
            }
            Some(url) => Ok(Some(Self::Url(url.trim().into()))),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_configuration_is_disabled_and_url_is_supported() {
        assert!(
            CesiumSource::from_values(None, None, None)
                .unwrap()
                .is_none()
        );
        assert!(matches!(
            CesiumSource::from_values(Some("https://example.org/tileset.json"), None, None)
                .unwrap(),
            Some(CesiumSource::Url(_))
        ));
    }
    #[test]
    fn ion_takes_precedence_without_falling_back_on_invalid_configuration() {
        assert!(matches!(
            CesiumSource::from_values(Some("https://example.org"), Some("42"), Some("test-token"))
                .unwrap(),
            Some(CesiumSource::Ion { asset_id: 42, .. })
        ));
        for asset in ["", "0", "-1", "not-an-id", "9223372036854775808"] {
            assert!(
                CesiumSource::from_values(
                    Some("https://example.org"),
                    Some(asset),
                    Some("test-token")
                )
                .is_err()
            );
        }
    }
    #[test]
    fn incomplete_configuration_is_rejected_without_leaking_secrets() {
        for (url, asset, token) in [
            (Some(" "), None, None),
            (None, Some("42"), None),
            (None, Some("42"), Some(" ")),
            (None, None, Some("secret-token")),
            (
                Some("https://example.org?secret"),
                Some("secret-id"),
                Some("secret-token"),
            ),
        ] {
            let err = CesiumSource::from_values(url, asset, token).err().unwrap();
            assert!(!err.to_string().contains("secret"));
            assert!(!format!("{err:?}").contains("secret"));
        }
    }
}
