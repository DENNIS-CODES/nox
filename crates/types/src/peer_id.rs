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

pub mod serde {
    use libp2p_identity::PeerId;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::str::FromStr;

    pub fn serialize<S>(value: &PeerId, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        value.to_base58().serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<PeerId, D::Error>
    where
        D: Deserializer<'de>,
    {
        let str = String::deserialize(deserializer)?;
        PeerId::from_str(&str).map_err(|e| {
            serde::de::Error::custom(format!("peer id deserialization failed for {e:?}"))
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::peer_id;
    use libp2p_identity::{Keypair, PeerId};
    use serde::{Deserialize, Serialize};
    use std::str::FromStr;

    #[test]
    fn peerid() {
        #[derive(Eq, PartialEq, Clone, Debug, Serialize, Deserialize)]
        struct Test {
            #[serde(
                serialize_with = "peer_id::serde::serialize",
                deserialize_with = "peer_id::serde::deserialize"
            )]
            peer_id_1: PeerId,
            #[serde(
                serialize_with = "peer_id::serde::serialize",
                deserialize_with = "peer_id::serde::deserialize"
            )]
            peer_id_2: PeerId,
        }

        let peer_id_1 = Keypair::generate_ed25519().public().to_peer_id();
        let peer_id_2 = PeerId::from_str("QmY28NSCefB532XbERtnKHadexGuNzAfYnh5fJk6qhLsSi").unwrap();

        let test = Test {
            peer_id_1,
            peer_id_2,
        };

        let serialized_test = serde_json::to_value(test.clone());
        assert!(
            serialized_test.is_ok(),
            "failed to serialize test struct: {}",
            serialized_test.err().unwrap()
        );

        let deserialized_test = serde_json::from_value::<Test>(serialized_test.unwrap());
        assert!(
            deserialized_test.is_ok(),
            "failed to deserialize test struct: {}",
            deserialized_test.err().unwrap()
        );
        assert_eq!(deserialized_test.unwrap(), test);
    }
}
