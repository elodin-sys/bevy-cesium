//! Portable Photo 3D resources. Access credentials exist only in the session.
use crate::{Error, Result};
use serde::Deserialize;
use std::collections::HashMap;
use url::Url;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    version: u32,
    pub ion_asset_id: i64,
    pub root: String,
    pub resources: HashMap<String, String>,
}

pub(crate) struct Package {
    pub base: Url,
    pub token: Option<String>,
    pub manifest: Option<Manifest>,
    pub credentials: Option<GoogleSession>,
}

pub(crate) struct GoogleSession {
    key: String,
    session: Option<String>,
}

pub(crate) enum Request {
    Local(Url),
    Cached { local: Url, remote: Url },
    Provider(Url),
}

impl Package {
    pub fn new(source: &Url, token: Option<&str>) -> Result<Self> {
        if !matches!(source.scheme(), "http" | "https" | "file")
            || !source.username().is_empty()
            || source.password().is_some()
            || source.query().is_some()
            || source.fragment().is_some()
            || token.is_some_and(|t| t.trim().is_empty() || t.contains(['\r', '\n']))
        {
            return Err(Error("Invalid prepared terrain source"));
        }
        Ok(Self {
            base: source
                .join(".")
                .map_err(|_| Error("Invalid package location"))?,
            token: token.map(str::to_owned),
            manifest: None,
            credentials: None,
        })
    }

    pub fn install(&mut self, bytes: &[u8]) -> Result<Url> {
        let manifest: Manifest = serde_json::from_slice(bytes)
            .map_err(|_| Error("Invalid prepared terrain manifest"))?;
        if manifest.version != 1 || manifest.ion_asset_id != 2275207 {
            return Err(Error("Unsupported prepared terrain provider or version"));
        }
        let root = Url::parse(&manifest.root).map_err(|_| Error("Invalid package root"))?;
        if canonical(&root)?.as_str() != manifest.root
            || root.path() != "/v1/3dtiles/root.json"
            || !manifest.resources.contains_key(&manifest.root)
        {
            return Err(Error("Invalid package root"));
        }
        for (remote, local) in &manifest.resources {
            let remote_url = Url::parse(remote).map_err(|_| Error("Invalid resource URL"))?;
            if canonical(&remote_url)?.as_str() != remote {
                return Err(Error("Package resource contains access parameters"));
            }
            self.local(local)?;
        }
        self.manifest = Some(manifest);
        Ok(root)
    }

    pub fn local(&self, relative: &str) -> Result<Url> {
        if relative.is_empty()
            || relative
                .split('/')
                .any(|s| s.is_empty() || s == "." || s == "..")
            || !relative
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"/._-".contains(&c))
        {
            return Err(Error("Invalid package asset path"));
        }
        self.base
            .join(relative)
            .map_err(|_| Error("Invalid package asset path"))
    }

    pub fn request(&self, url: &Url) -> Result<Request> {
        if (url.origin() == self.base.origin()
            || url.scheme() == "file" && self.base.scheme() == "file")
            && url.scheme() == self.base.scheme()
            && url.path().starts_with(self.base.path())
            && url.query().is_none()
            && url.fragment().is_none()
            && url.username().is_empty()
            && url.password().is_none()
        {
            let relative = &url.path()[self.base.path().len()..];
            return self.local(relative).map(Request::Local);
        }
        let url = canonical(url)?;
        if let Some(path) = self
            .manifest
            .as_ref()
            .and_then(|m| m.resources.get(url.as_str()))
        {
            let local = self.local(path)?;
            return Ok(if self.token.is_some() {
                Request::Cached { local, remote: url }
            } else {
                Request::Local(local)
            });
        }
        if self.token.is_none() {
            return Err(Error("Terrain is not in the offline package"));
        }
        Ok(Request::Provider(url))
    }
}

pub(crate) fn canonical(url: &Url) -> Result<Url> {
    if !crate::google_url(url)
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(Error("Unsupported prepared terrain resource origin"));
    }
    let mut pairs: Vec<_> = url
        .query_pairs()
        .filter(|(k, _)| k != "key" && k != "session")
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    pairs.sort();
    let mut result = url.clone();
    result.set_query(None);
    if !pairs.is_empty() {
        result.query_pairs_mut().extend_pairs(pairs);
    }
    Ok(result)
}

impl GoogleSession {
    pub fn from_root(url: &Url, bytes: &[u8]) -> Result<Self> {
        canonical(url)?;
        let key = url
            .query_pairs()
            .find(|(k, _)| k == "key")
            .map(|(_, v)| v.into_owned())
            .filter(|s| !s.is_empty())
            .ok_or(Error("Missing Photo 3D access key"))?;
        let document: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|_| Error("Invalid Photo 3D root"))?;
        fn find(value: &serde_json::Value, base: &Url) -> Option<String> {
            match value {
                serde_json::Value::Object(object) => {
                    for (k, v) in object {
                        if matches!(k.as_str(), "uri" | "url")
                            && let Some(uri) = v.as_str()
                            && let Ok(url) = base.join(uri)
                            && crate::google_url(&url)
                            && let Some((_, session)) =
                                url.query_pairs().find(|(k, _)| k == "session")
                        {
                            return Some(session.into_owned());
                        }
                        if let Some(session) = find(v, base) {
                            return Some(session);
                        }
                    }
                    None
                }
                serde_json::Value::Array(a) => a.iter().find_map(|v| find(v, base)),
                _ => None,
            }
        }
        Ok(Self {
            key,
            session: find(&document, url),
        })
    }

    pub fn authorize(&self, url: &Url) -> Result<Url> {
        let mut url = canonical(url)?;
        url.query_pairs_mut().append_pair("key", &self.key);
        if let Some(session) = &self.session {
            url.query_pairs_mut().append_pair("session", session);
        }
        Ok(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const ROOT: &str = "https://tile.googleapis.com/v1/3dtiles/root.json";
    #[test]
    fn cache_hits_precede_provider_and_offline_rejects_external_resources() {
        let mut package = Package::new(
            &Url::parse("https://assets.example/f9/tileset.json").unwrap(),
            Some("unused"),
        )
        .unwrap();
        let manifest = serde_json::json!({"version":1,"ion_asset_id":2275207,"root":ROOT,"resources":{ROOT:"source/root.json"}});
        package
            .install(&serde_json::to_vec(&manifest).unwrap())
            .unwrap();
        let root = Url::parse(&format!("{ROOT}?key=old&session=old")).unwrap();
        assert!(matches!(
            package.request(&root).unwrap(),
            Request::Cached { .. }
        ));
        let missing = Url::parse("https://tile.googleapis.com/v1/3dtiles/missing.glb").unwrap();
        assert!(matches!(
            package.request(&missing).unwrap(),
            Request::Provider(_)
        ));
        package.token = None;
        assert!(package.request(&missing).is_err());
        for external in [
            "https://other.example/x",
            "https://assets.example/private/file",
            "http://assets.example/f9/a.glb",
            "https://assets.example/f9/%2e%2e/private",
        ] {
            assert!(package.request(&Url::parse(external).unwrap()).is_err());
        }
    }

    #[test]
    fn paths_and_provider_credentials_cannot_escape_package() {
        let package = Package::new(
            &Url::parse("https://assets.example/f9/tileset.json").unwrap(),
            None,
        )
        .unwrap();
        for path in [
            "../secret",
            "/secret",
            "a/../../x",
            "a%2fb",
            "https://external/a",
            "a?key=x",
            "a\\b",
        ] {
            assert!(package.local(path).is_err());
        }
        let root = Url::parse(&format!("{ROOT}?key=fresh")).unwrap();
        let credentials = GoogleSession::from_root(
            &root,
            br#"{"root":{"content":{"uri":"a.json?session=new"}}}"#,
        )
        .unwrap();
        let authorized = credentials
            .authorize(
                &Url::parse("https://tile.googleapis.com/v1/3dtiles/a.glb?session=old&key=old")
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(authorized.query(), Some("key=fresh&session=new"));
        assert!(
            credentials
                .authorize(&Url::parse("https://external/a").unwrap())
                .is_err()
        );
    }
}
