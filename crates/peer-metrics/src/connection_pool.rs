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
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::{exponential_buckets, Histogram};
use prometheus_client::registry::Registry;

#[derive(Clone)]
pub struct ConnectionPoolMetrics {
    pub received_particles: Family<ParticleLabel, Counter>,
    pub particle_sizes: Family<ParticleLabel, Histogram>,
    pub connected_peers: Gauge,
    pub particle_queue_size: Gauge,
}

impl ConnectionPoolMetrics {
    pub fn new(registry: &mut Registry) -> Self {
        let sub_registry = registry.sub_registry_with_prefix("connection_pool");

        let received_particles = Family::default();
        sub_registry.register(
            "received_particles",
            "Number of particles received from the network (not unique)",
            received_particles.clone(),
        );

        // from 100 bytes to 100 MB
        let particle_sizes: Family<_, _> =
            Family::new_with_constructor(|| Histogram::new(exponential_buckets(100.0, 10.0, 7)));
        sub_registry.register(
            "particle_sizes",
            "Distribution of particle data sizes",
            particle_sizes.clone(),
        );

        let connected_peers = Gauge::default();
        sub_registry.register(
            "connected_peers",
            "Number of peers we have connections to at a given moment",
            connected_peers.clone(),
        );

        let particle_queue_size = Gauge::default();
        sub_registry.register(
            "particle_queue_size",
            "Size of a particle queue in connection pool",
            particle_queue_size.clone(),
        );

        Self {
            received_particles,
            particle_sizes,
            connected_peers,
            particle_queue_size,
        }
    }

    pub fn incoming_particle(&self, particle_id: &str, queue_len: i64, particle_len: f64) {
        self.particle_queue_size.set(queue_len);
        let label = ParticleLabel {
            particle_type: ParticleType::from_particle(particle_id),
        };
        self.received_particles.get_or_create(&label).inc();
        self.particle_sizes
            .get_or_create(&label)
            .observe(particle_len);
    }
}
