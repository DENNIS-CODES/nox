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

use std::time::Duration;

use prometheus_client::encoding::{EncodeLabelSet, EncodeLabelValue};
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::Histogram;
use prometheus_client::registry::Registry;

use crate::execution_time_buckets;

#[derive(Copy, Clone, Debug, EncodeLabelValue, Hash, Eq, PartialEq)]
pub enum FunctionKind {
    Service,
    ParticleFunction,
    // Function call failed early
    NotHappened,
}

#[derive(EncodeLabelSet, Hash, Clone, Eq, PartialEq, Debug)]
pub struct FunctionKindLabel {
    function_kind: FunctionKind,
}

#[derive(Clone)]
pub struct ParticleExecutorMetrics {
    pub interpretation_time_sec: Family<WorkerLabel, Histogram>,
    pub interpretation_successes: Family<WorkerLabel, Counter>,
    pub interpretation_failures: Family<WorkerLabel, Counter>,
    pub total_actors_mailbox: Family<WorkerLabel, Gauge>,
    pub alive_actors: Family<WorkerLabel, Gauge>,
    service_call_time_sec: Family<FunctionKindLabel, Histogram>,
    service_call_success: Family<FunctionKindLabel, Counter>,
    service_call_failure: Family<FunctionKindLabel, Counter>,
}

#[derive(EncodeLabelSet, Debug, Clone, Hash, Eq, PartialEq)]
pub struct WorkerLabel {
    worker_type: WorkerType,
    peer_id: String,
}

impl WorkerLabel {
    pub fn new(worker_type: WorkerType, peer_id: String) -> Self {
        Self {
            worker_type,
            peer_id,
        }
    }
}

#[derive(EncodeLabelValue, Debug, Clone, Hash, Eq, PartialEq)]
pub enum WorkerType {
    Worker,
    Host,
}

impl ParticleExecutorMetrics {
    pub fn new(registry: &mut Registry) -> Self {
        let sub_registry = registry.sub_registry_with_prefix("particle_executor");

        let interpretation_time_sec: Family<WorkerLabel, Histogram> =
            Family::new_with_constructor(|| Histogram::new(execution_time_buckets()));
        sub_registry.register(
            "interpretation_time_sec",
            "Distribution of time it took to run the interpreter once",
            interpretation_time_sec.clone(),
        );

        let call_time_sec = Histogram::new(execution_time_buckets());
        sub_registry.register(
            "avm_call_time_sec",
            "Distribution of time it took to run the avm call (interpretation + saving the particle on disk) once",
            call_time_sec.clone(),
        );

        let interpretation_successes = Family::default();
        sub_registry.register(
            "interpretation_successes",
            "Number successfully interpreted particles",
            interpretation_successes.clone(),
        );

        let interpretation_failures = Family::default();
        sub_registry.register(
            "interpretation_failures",
            "Number of failed particle interpretations",
            interpretation_failures.clone(),
        );

        let total_actors_mailbox: Family<WorkerLabel, Gauge> =
            Family::new_with_constructor(Gauge::default);
        sub_registry.register(
            "total_actors_mailbox",
            "Cumulative sum of all actors' mailboxes",
            total_actors_mailbox.clone(),
        );
        let alive_actors: Family<WorkerLabel, Gauge> = Family::new_with_constructor(Gauge::default);
        sub_registry.register(
            "alive_actors",
            "Number of currently alive actors (1 particle id = 1 actor)",
            alive_actors.clone(),
        );

        let service_call_time_sec: Family<_, _> =
            Family::new_with_constructor(|| Histogram::new(execution_time_buckets()));
        sub_registry.register(
            "service_call_time_sec",
            "Distribution of time it took to execute a single service or builtin call",
            service_call_time_sec.clone(),
        );
        let service_call_success = Family::default();
        sub_registry.register(
            "service_call_success",
            "Number of succeeded service calls",
            service_call_success.clone(),
        );
        let service_call_failure = Family::default();
        sub_registry.register(
            "service_call_failure",
            "Number of failed service calls",
            service_call_failure.clone(),
        );

        Self {
            interpretation_time_sec,
            interpretation_successes,
            interpretation_failures,
            total_actors_mailbox,
            alive_actors,
            service_call_time_sec,
            service_call_success,
            service_call_failure,
        }
    }

    pub fn service_call(&self, success: bool, kind: FunctionKind, run_time: Option<Duration>) {
        let label = FunctionKindLabel {
            function_kind: kind,
        };

        if success {
            self.service_call_success.get_or_create(&label).inc();
        } else {
            self.service_call_failure.get_or_create(&label).inc();
        }
        if let Some(run_time) = run_time {
            self.service_call_time_sec
                .get_or_create(&label)
                .observe(run_time.as_secs_f64())
        }
    }
}
