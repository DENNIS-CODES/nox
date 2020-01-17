/*
 * Copyright 2019 Fluence Labs Limited
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

use serde::{Deserialize, Serialize};

/// Relay message is just a data that need to be relayed from src node to dst node.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct RelayMessage {
    pub src: Vec<u8>,
    pub dst: Vec<u8>,
    pub data: Vec<u8>,
}
