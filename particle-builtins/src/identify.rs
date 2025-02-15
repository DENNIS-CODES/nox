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

use libp2p::core::Multiaddr;
use serde::Serialize;

#[derive(Serialize, Clone, Debug)]
pub struct NodeInfo {
    pub external_addresses: Vec<Multiaddr>,
    pub node_version: &'static str,
    pub air_version: &'static str,
    pub spell_version: String,
    pub allowed_binaries: Vec<String>,
}
