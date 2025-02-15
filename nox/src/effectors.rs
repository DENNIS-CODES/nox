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

use futures::{stream::iter, StreamExt};
use tracing::instrument;

use aquamarine::RemoteRoutingEffects;
use particle_protocol::Particle;

use crate::connectivity::Connectivity;

#[derive(Clone)]
pub struct Effectors {
    pub connectivity: Connectivity,
}

impl Effectors {
    pub fn new(connectivity: Connectivity) -> Self {
        Self { connectivity }
    }

    /// Perform effects that Aquamarine instructed us to
    #[instrument(level = tracing::Level::INFO, skip_all)]
    pub async fn execute(self, effects: RemoteRoutingEffects) {
        let particle: &Particle = effects.particle.as_ref();
        if particle.is_expired() {
            tracing::info!(target: "expired", particle_id = particle.id, "Particle is expired");
            return;
        }

        // take every next peers, and try to send particle there concurrently
        let nps = iter(effects.next_peers);
        let particle = &effects.particle;
        let connectivity = self.connectivity.clone();
        nps.for_each_concurrent(None, move |target| {
            let connectivity = connectivity.clone();
            let particle = particle.clone();
            async move {
                // resolve contact
                if let Some(contact) = connectivity
                    .resolve_contact(target, particle.as_ref())
                    .await
                {
                    // forward particle
                    let sent = connectivity.send(contact, particle).await;
                    if sent {
                        // resolved and sent, exit
                    }
                }
            }
        })
        .await;
    }
}
