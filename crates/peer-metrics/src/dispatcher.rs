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

use crate::{ParticleLabel, ParticleType};
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::registry::Registry;

#[derive(Clone)]
pub struct DispatcherMetrics {
    pub expired_particles: Family<ParticleLabel, Counter>,
}

impl DispatcherMetrics {
    pub fn new(registry: &mut Registry, _parallelism: Option<usize>) -> Self {
        let sub_registry = registry.sub_registry_with_prefix("dispatcher");

        // TODO: prometheus doesn't parse this Info metric. Find a way to make it work.
        //       Gauge would work, but maybe it's possible to make Info work as well?
        // // NOTE: it MUST by a Vec of (String, String) or it would generate gibberish!
        // let parallelism: Info<Vec<(String, String)>> = Info::new(vec![(
        //     "particle_parallelism".to_string(),
        //     parallelism.map_or("unlimited".to_string(), |p| p.to_string()),
        // )]);
        // sub_registry.register(
        //     "particle_parallelism",
        //     "limit of simultaneously processed particles",
        //     Box::new(parallelism),
        // );

        let expired_particles = Family::default();
        sub_registry.register(
            "particles_expired",
            "Number of particles expired by TTL",
            expired_particles.clone(),
        );

        DispatcherMetrics { expired_particles }
    }

    pub fn particle_expired(&self, particle_id: &str) {
        self.expired_particles
            .get_or_create(&ParticleLabel {
                particle_type: ParticleType::from_particle(particle_id),
            })
            .inc();
    }
}
