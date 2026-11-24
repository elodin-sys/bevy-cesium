use std::time::SystemTime;

/// Failure context deliberately excludes URLs, credentials and response bodies.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Error {
    Message(&'static str),
    Http {
        kind: HttpError,
        source: HttpSource,
        retry_after: Option<f64>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpError {
    Status(u16),
    Network,
    Timeout,
    Redirect,
    Body,
    /// Browsers do not expose the distinction between network and CORS errors.
    BrowserNetwork,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpSource {
    Source,
    Package,
    Provider,
}

impl Error {
    pub(crate) fn http(kind: HttpError) -> Self {
        Self::Http {
            kind,
            source: HttpSource::Source,
            retry_after: None,
        }
    }

    pub(crate) fn with_source(self, source: HttpSource) -> Self {
        match self {
            Self::Http {
                kind, retry_after, ..
            } => Self::Http {
                kind,
                source,
                retry_after,
            },
            other => other,
        }
    }

    pub(crate) fn status(self) -> Option<u16> {
        match self {
            Self::Http {
                kind: HttpError::Status(status),
                ..
            } => Some(status),
            _ => None,
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::Message(message) => message,
            Self::Http { kind, .. } => match kind {
                HttpError::Status(401) => "HTTP unauthorized (401)",
                HttpError::Status(403) => "HTTP access denied (403)",
                HttpError::Status(404) => "HTTP not found (404)",
                HttpError::Status(408) => "HTTP request timeout (408)",
                HttpError::Status(429) => "HTTP rate limited (429)",
                HttpError::Status(500) => "HTTP server error (500)",
                HttpError::Status(502) => "HTTP bad gateway (502)",
                HttpError::Status(503) => "HTTP service unavailable (503)",
                HttpError::Status(504) => "HTTP gateway timeout (504)",
                HttpError::Status(_) => "HTTP unexpected status",
                HttpError::Network => "HTTP network error",
                HttpError::Timeout => "HTTP timeout",
                HttpError::Redirect => "HTTP redirect rejected",
                HttpError::Body => "HTTP body read failed",
                HttpError::BrowserNetwork => "HTTP network or CORS error",
            },
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Message(message) => f.write_str(message),
            Self::Http {
                kind,
                source,
                retry_after,
            } => {
                match kind {
                    HttpError::Status(status) => write!(f, "HTTP {status}"),
                    _ => f.write_str(self.message()),
                }?;
                write!(
                    f,
                    " ({})",
                    match source {
                        HttpSource::Source => "source",
                        HttpSource::Package => "prepared package",
                        HttpSource::Provider => "provider",
                    }
                )?;
                if let Some(seconds) = retry_after {
                    write!(f, "; retry after {seconds:.1}s")?;
                }
                Ok(())
            }
        }
    }
}
impl std::error::Error for Error {}

pub(crate) fn retry_after(value: Option<&str>, now: SystemTime) -> Option<f64> {
    let value = value?.trim();
    if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
        return value.parse::<u64>().ok().map(|n| n as f64);
    }
    httpdate::parse_http_date(value)
        .ok()
        .map(|date| date.duration_since(now).unwrap_or_default().as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_accepts_seconds_and_http_dates() {
        let now = httpdate::parse_http_date("Wed, 21 Oct 2015 07:28:00 GMT").unwrap();
        assert_eq!(retry_after(Some("120"), now), Some(120.));
        assert_eq!(
            retry_after(Some("Wed, 21 Oct 2015 07:30:00 GMT"), now),
            Some(120.)
        );
        assert_eq!(
            retry_after(Some("Wed, 21 Oct 2015 07:27:00 GMT"), now),
            Some(0.)
        );
        for value in [None, Some("NaN"), Some("-1"), Some("1.5"), Some("")] {
            assert_eq!(retry_after(value, now), None);
        }
    }
}
