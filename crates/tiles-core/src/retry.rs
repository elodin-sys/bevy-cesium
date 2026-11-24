use crate::{Error, HttpError, HttpSource};

pub(crate) fn delay(error: Error, attempt: u32, id: usize) -> Option<f64> {
    let Error::Http {
        kind, retry_after, ..
    } = error
    else {
        return None;
    };
    if !matches!(
        kind,
        HttpError::Status(408 | 429 | 500..=599)
            | HttpError::Network
            | HttpError::Timeout
            | HttpError::Body
            | HttpError::BrowserNetwork
    ) {
        return None;
    }
    // Stable jitter spreads tile retries without an RNG or unbounded growth.
    let base = (10. * 2f64.powi(attempt.min(4) as i32)).min(120.);
    let jitter = ((id.wrapping_mul(37).wrapping_add(attempt as usize * 17)) % 21) as f64 / 100.;
    Some((base * (1. + jitter)).max(retry_after.unwrap_or(0.)))
}

/// Shared by a loader's workers. A provider throttle must not block local reads.
#[derive(Default)]
pub(crate) struct Cooldowns([Option<(f64, Error)>; 3]);

impl Cooldowns {
    pub(crate) fn check(&self, source: HttpSource, now: f64) -> crate::Result<()> {
        if let Some((deadline, Error::Http { kind, .. })) = self.0[source as usize]
            && now < deadline
        {
            return Err(Error::Http {
                kind,
                source,
                retry_after: Some(deadline - now),
            });
        }
        Ok(())
    }

    pub(crate) fn observe(&mut self, error: Error, now: f64) {
        if let Error::Http {
            kind: HttpError::Status(429 | 503),
            source,
            retry_after,
        } = error
        {
            let deadline = now + retry_after.unwrap_or(10.).max(10.);
            let slot = &mut self.0[source as usize];
            if slot.is_none_or(|(old, _)| deadline > old) {
                *slot = Some((deadline, error));
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct FailureLog {
    count: u64,
    sample: Option<(usize, Error)>,
    next_report: f64,
}

impl FailureLog {
    pub(crate) fn record(&mut self, id: usize, error: Error) {
        self.count += 1;
        self.sample = Some((id, error));
    }

    pub(crate) fn take(&mut self, now: f64) -> Option<(u64, usize, Error)> {
        if now < self.next_report {
            return None;
        }
        let (id, error) = self.sample.take()?;
        self.next_report = now + 5.;
        Some((std::mem::take(&mut self.count), id, error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_back_off_and_never_beat_retry_after() {
        let error = Error::http(HttpError::Status(429));
        let delays: Vec<_> = (0..10).map(|i| delay(error, i, 0).unwrap()).collect();
        assert!(delays[1] > delays[0] && delays[2] > delays[1]);
        assert!(delays.iter().all(|d| *d <= 144.));
        let hinted = Error::Http {
            kind: HttpError::Status(503),
            source: HttpSource::Provider,
            retry_after: Some(600.),
        };
        assert_eq!(delay(hinted, 0, 1), Some(600.));
        for status in [301, 401, 403, 404, 422] {
            assert_eq!(delay(Error::http(HttpError::Status(status)), 0, 0), None);
        }
        assert_eq!(delay(Error::Message("Invalid glTF"), 0, 0), None);
        assert!(delay(Error::http(HttpError::Timeout), 0, 0).is_some());
    }

    #[test]
    fn provider_cooldown_is_shared_without_blocking_packages() {
        let mut cooldowns = Cooldowns::default();
        cooldowns.observe(
            Error::Http {
                kind: HttpError::Status(429),
                source: HttpSource::Provider,
                retry_after: Some(60.),
            },
            5.,
        );
        assert!(cooldowns.check(HttpSource::Provider, 64.9).is_err());
        assert!(cooldowns.check(HttpSource::Package, 6.).is_ok());
        assert!(cooldowns.check(HttpSource::Provider, 65.).is_ok());
    }

    #[test]
    fn repeated_errors_are_counted_instead_of_logged_per_tile() {
        let mut log = FailureLog::default();
        let error = Error::http(HttpError::Status(503));
        log.record(0, error);
        assert_eq!(log.take(0.).unwrap().0, 1);
        for id in 1..1000 {
            log.record(id, error);
        }
        assert!(log.take(4.9).is_none());
        assert_eq!(log.take(5.), Some((999, 999, error)));
        assert!(log.take(10.).is_none());
    }
}
