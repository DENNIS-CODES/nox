/*
 * Copyright 2024 Fluence DAO
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use crate::api::PeerEventType;
use fluence_spell_dtos::trigger_config::{
    ClockConfig, ConnectionPoolConfig, TriggerConfig as UserTriggerConfig,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;

const MAX_PERIOD_YEAR: u32 = 100;

/// Max period is 100 years in secs: 60 sec * 60 min * 24 hours * 365 days * 100 years
pub const MAX_PERIOD_SEC: u32 = 60 * 60 * 24 * 365 * MAX_PERIOD_YEAR;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error(
        "invalid config: period is too big. Max period is {} years (or approx {} seconds)",
        MAX_PERIOD_YEAR,
        MAX_PERIOD_SEC
    )]
    InvalidPeriod,
    #[error("invalid config: end_sec is less than start_sec or in the past")]
    InvalidEndSec,
}

/// Convert timestamp to std::time::Instant.
/// Fails if the timestamp is in the past or overflow occurred which actually shouldn't happen.
fn to_instant(timestamp: u64) -> Option<Instant> {
    let target_time = UNIX_EPOCH.checked_add(Duration::from_secs(timestamp))?;
    let duration = target_time.duration_since(SystemTime::now()).ok()?;
    Instant::now().checked_add(duration)
}

/// Convert user-friendly config to event-bus-friendly config, validating it in the process.
pub fn from_user_config(
    user_config: &UserTriggerConfig,
) -> Result<Option<SpellTriggerConfigs>, ConfigError> {
    let mut triggers = Vec::new();

    // ClockConfig is considered empty if `start_sec` is zero. In this case the content of other fields are ignored.
    if user_config.clock.start_sec != 0 {
        let timer_config = from_clock_config(&user_config.clock)?;
        triggers.push(TriggerConfig::Timer(timer_config));
    }

    if let Some(peer_event_config) = from_connection_config(&user_config.connections) {
        triggers.push(TriggerConfig::PeerEvent(peer_event_config));
    }

    let cfg = if !triggers.is_empty() {
        Some(SpellTriggerConfigs { triggers })
    } else {
        None
    };
    Ok(cfg)
}

fn from_connection_config(connection_config: &ConnectionPoolConfig) -> Option<PeerEventConfig> {
    let mut pool_events = Vec::with_capacity(2);
    if connection_config.connect {
        pool_events.push(PeerEventType::Connected);
    }
    if connection_config.disconnect {
        pool_events.push(PeerEventType::Disconnected);
    }
    if pool_events.is_empty() {
        None
    } else {
        Some(PeerEventConfig {
            events: pool_events,
        })
    }
}

fn from_clock_config(clock: &ClockConfig) -> Result<TimerConfig, ConfigError> {
    // Check the upper bound of period.
    if clock.period_sec > MAX_PERIOD_SEC {
        return Err(ConfigError::InvalidPeriod);
    }

    let end_at = if clock.end_sec == 0 {
        // If `end_sec` is 0 then the spell will be triggered forever.
        None
    } else if clock.end_sec < clock.start_sec {
        // The config is invalid `end_sec` is less than `start_sec`
        return Err(ConfigError::InvalidEndSec);
    } else {
        // If conversion fails that means that `end_sec` is in the past.
        match to_instant(clock.end_sec as u64) {
            Some(end_at) => Some(end_at),
            None => return Err(ConfigError::InvalidEndSec),
        }
    };

    // Start now if the start time is in the past
    let start_at = to_instant(clock.start_sec as u64).unwrap_or_else(Instant::now);

    // If period is 0 then the timer will be triggered only once at start_sec and then stopped.
    let config = if clock.period_sec == 0 {
        // Should we ignore checking end_sec if period is 0?
        TimerConfig::oneshot(start_at)
    } else {
        TimerConfig::periodic(
            Duration::from_secs(clock.period_sec as u64),
            start_at,
            end_at,
        )
    };

    Ok(config)
}

#[derive(Debug, Clone)]
pub struct SpellTriggerConfigs {
    pub(crate) triggers: Vec<TriggerConfig>,
}

impl SpellTriggerConfigs {
    pub fn into_rescheduled(self) -> Option<Self> {
        let new_triggers: Vec<TriggerConfig> = self
            .triggers
            .into_iter()
            .filter_map(|trigger| trigger.into_rescheduled())
            .collect::<_>();
        if new_triggers.is_empty() {
            None
        } else {
            Some(SpellTriggerConfigs {
                triggers: new_triggers,
            })
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum TriggerConfig {
    Timer(TimerConfig),
    PeerEvent(PeerEventConfig),
}

impl TriggerConfig {
    pub fn into_rescheduled(self) -> Option<TriggerConfig> {
        if let TriggerConfig::Timer(c) = self {
            c.into_rescheduled().map(TriggerConfig::Timer)
        } else {
            // Peer events can't stop being relevant
            Some(self)
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TimerConfig {
    pub(crate) period: Duration,
    pub(crate) start_at: Instant,
    pub(crate) end_at: Option<Instant>,
}

impl TimerConfig {
    pub(crate) fn periodic(period: Duration, start_at: Instant, end_at: Option<Instant>) -> Self {
        Self {
            period,
            start_at,
            end_at,
        }
    }

    pub(crate) fn oneshot(start_at: Instant) -> Self {
        // We set `end_at` to `start_at` to make sure that on rescheduling the spell will be stopped.
        // I'm not sure maybe it's better to move this piece of code inside the bus module.
        Self {
            period: Duration::ZERO,
            start_at,
            end_at: Some(start_at),
        }
    }

    pub fn into_rescheduled(self) -> Option<TimerConfig> {
        let now = std::time::Instant::now();
        // Check that the spell is ended
        if self.end_at.map(|end_at| end_at <= now).unwrap_or(false) {
            return None;
        }
        // Check that the spell is oneshot and is ended
        if self.period == Duration::ZERO && self.start_at < now {
            return None;
        }
        Some(self)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PeerEventConfig {
    pub(crate) events: Vec<PeerEventType>,
}

#[cfg(test)]
mod trigger_config_tests {
    use crate::api::PeerEventType;
    use crate::config::{PeerEventConfig, SpellTriggerConfigs, TimerConfig, TriggerConfig};
    use std::assert_matches::assert_matches;
    use std::time::{Duration, Instant};

    #[test]
    fn test_reschedule_ok_periodic() {
        let now = Instant::now();
        // start in the past
        let start_at = now - Duration::from_secs(120);
        let timer_config = TimerConfig::periodic(Duration::from_secs(1), start_at, None);

        let rescheduled = timer_config.into_rescheduled();
        assert!(
            rescheduled.is_some(),
            "should be rescheduled since the config is periodic"
        );
    }

    #[test]
    fn test_reschedule_ok_periodic_end_future() {
        let now = Instant::now();
        // start in the past
        let start_at = now - Duration::from_secs(120);
        let end_at = now + Duration::from_secs(120);
        let timer_config = TimerConfig::periodic(Duration::from_secs(1), start_at, Some(end_at));

        let rescheduled = timer_config.into_rescheduled();
        assert!(
            rescheduled.is_some(),
            "should be rescheduled since the config is periodic and doesn't end soon"
        );
    }

    #[test]
    fn test_reschedule_ok_oneshot_start_future() {
        let now = Instant::now();
        // start in the past
        let start_at = now + Duration::from_secs(120);
        let timer_config = TimerConfig::oneshot(start_at);

        let rescheduled = timer_config.into_rescheduled();
        assert!(
            rescheduled.is_some(),
            "should be rescheduled since the oneshot config start in the future"
        );
    }

    #[test]
    fn test_reschedule_fail_ended() {
        let now = Instant::now();
        // start in the past
        let start_at = now - Duration::from_secs(120);
        let timer_config = TimerConfig::oneshot(start_at);

        let rescheduled = timer_config.into_rescheduled();
        assert!(
            rescheduled.is_none(),
            "shouldn't be rescheduled since the config is ended"
        );
    }

    #[test]
    fn test_reschedule_fail_oneshot_executed() {
        let now = Instant::now();
        // start in the past
        let start_at = now - Duration::from_secs(120);
        let mut timer_config = TimerConfig::oneshot(start_at);
        // oneshot config that ends in the future (not in use bth)
        timer_config.end_at = Some(now + Duration::from_secs(120));

        let rescheduled = timer_config.into_rescheduled();
        assert!(
            rescheduled.is_none(),
            "shouldn't be rescheduled since the oneshot config already shot"
        );
    }

    #[test]
    fn test_peer_events() {
        let peer_events = vec![PeerEventType::Connected, PeerEventType::Disconnected];
        let peer_event_config = PeerEventConfig {
            events: peer_events,
        };
        let trigger_config = TriggerConfig::PeerEvent(peer_event_config);
        let rescheduled = trigger_config.into_rescheduled();
        assert!(
            rescheduled.is_some(),
            "should be rescheduled since the config is periodic"
        );
    }

    // Test that ended configs are filtered out after rescheduling
    #[test]
    fn test_both_types_ended() {
        let peer_events = vec![PeerEventType::Connected, PeerEventType::Disconnected];
        let peer_event_config = PeerEventConfig {
            events: peer_events,
        };
        let peer_trigger_config = TriggerConfig::PeerEvent(peer_event_config);
        let timer_config = TriggerConfig::Timer(TimerConfig::oneshot(
            Instant::now() - Duration::from_secs(120),
        ));
        let spell_trigger_config = SpellTriggerConfigs {
            triggers: vec![peer_trigger_config, timer_config],
        };
        let rescheduled = spell_trigger_config.into_rescheduled();
        assert!(
            rescheduled.is_some(),
            "should be rescheduled since the config is periodic"
        );
        assert_matches!(
            rescheduled.unwrap().triggers[..],
            [TriggerConfig::PeerEvent(_)]
        );
    }

    #[test]
    fn test_both_types_ok() {
        let peer_events = vec![PeerEventType::Connected, PeerEventType::Disconnected];
        let peer_event_config = PeerEventConfig {
            events: peer_events,
        };
        let peer_trigger_config = TriggerConfig::PeerEvent(peer_event_config);
        let timer_config = TriggerConfig::Timer(TimerConfig::periodic(
            Duration::from_secs(1),
            Instant::now(),
            None,
        ));
        let spell_trigger_config = SpellTriggerConfigs {
            triggers: vec![peer_trigger_config, timer_config],
        };
        let rescheduled = spell_trigger_config.into_rescheduled();
        assert!(
            rescheduled.is_some(),
            "should be rescheduled since the config is periodic"
        );
        assert_matches!(
            rescheduled.unwrap().triggers[..],
            [TriggerConfig::PeerEvent(_), TriggerConfig::Timer(_)]
        );
    }
}
