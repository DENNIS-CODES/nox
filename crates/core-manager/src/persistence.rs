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

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use ccp_shared::types::{LogicalCoreId, PhysicalCoreId, CUID};
use futures::StreamExt;
use hex_utils::serde_as::Hex;
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use tokio::sync::mpsc::Receiver;
use tokio_stream::wrappers::ReceiverStream;

use crate::errors::PersistError;
use crate::types::WorkType;
use crate::CoreManager;

pub trait PersistentCoreManagerFunctions {
    fn persist(&self) -> Result<(), PersistError>;
}

pub struct PersistenceTask {
    receiver: Receiver<()>,
}

impl PersistenceTask {
    pub(crate) fn new(receiver: Receiver<()>) -> Self {
        Self { receiver }
    }
}

impl PersistenceTask {
    async fn process_events<Src>(stream: Src, core_manager: Arc<CoreManager>)
    where
        Src: futures::Stream<Item = ()> + Unpin + Send + Sync + 'static,
    {
        let core_manager = core_manager.clone();
        // We are not interested in the content of the event
        // We are waiting for the event to initiate the persistence process
        stream.for_each(move |_| {
            let core_manager = core_manager.clone();
            async move {
                tokio::task::spawn_blocking(move || {
                    if let CoreManager::Persistent(manager) = core_manager.as_ref() {
                        let result = manager.persist();
                        match result {
                            Ok(_) => {
                                tracing::debug!(target: "core-manager", "Core state was persisted");
                            }
                            Err(err) => {
                                tracing::warn!(target: "core-manager", "Failed to save core state {err}");
                            }
                        }
                    }
                })
                    .await
                    .expect("Could not spawn persist task")
            }
        }).await;
    }

    pub async fn run(self, core_manager: Arc<CoreManager>) {
        let stream = ReceiverStream::from(self.receiver);

        tokio::task::Builder::new()
            .name("core-manager-persist")
            .spawn(Self::process_events(stream, core_manager))
            .expect("Could not spawn persist task");
    }
}

#[serde_as]
#[derive(Serialize, Deserialize)]
pub struct PersistentCoreManagerState {
    pub cores_mapping: Vec<(PhysicalCoreId, LogicalCoreId)>,
    pub system_cores: Vec<PhysicalCoreId>,
    pub available_cores: Vec<PhysicalCoreId>,
    #[serde_as(as = "Vec<(_, Hex)>")]
    pub unit_id_mapping: Vec<(PhysicalCoreId, CUID)>,
    #[serde_as(as = "Vec<(Hex, _)>")]
    pub work_type_mapping: Vec<(CUID, WorkType)>,
}

impl PersistentCoreManagerState {
    pub fn persist(&self, file_path: &Path) -> Result<(), PersistError> {
        let toml = toml::to_string_pretty(&self)
            .map_err(|err| PersistError::SerializationError { err })?;
        let mut file = File::create(file_path).map_err(|err| PersistError::IoError { err })?;
        file.write(toml.as_bytes())
            .map_err(|err| PersistError::IoError { err })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::persistence::PersistentCoreManagerState;
    use crate::types::WorkType;
    use ccp_shared::types::{LogicalCoreId, PhysicalCoreId, CUID};
    use hex::FromHex;

    #[test]
    fn test_serde() {
        let init_id_1 =
            <CUID>::from_hex("54ae1b506c260367a054f80800a545f23e32c6bc4a8908c9a794cb8dad23e5ea")
                .unwrap();
        let persistent_state = PersistentCoreManagerState {
            cores_mapping: vec![
                (PhysicalCoreId::new(1), LogicalCoreId::new(1)),
                (PhysicalCoreId::new(1), LogicalCoreId::new(2)),
                (PhysicalCoreId::new(2), LogicalCoreId::new(3)),
                (PhysicalCoreId::new(2), LogicalCoreId::new(4)),
                (PhysicalCoreId::new(3), LogicalCoreId::new(5)),
                (PhysicalCoreId::new(3), LogicalCoreId::new(6)),
                (PhysicalCoreId::new(4), LogicalCoreId::new(7)),
                (PhysicalCoreId::new(4), LogicalCoreId::new(8)),
            ],
            system_cores: vec![PhysicalCoreId::new(1)],
            available_cores: vec![PhysicalCoreId::new(2), PhysicalCoreId::new(3)],
            unit_id_mapping: vec![(PhysicalCoreId::new(4), init_id_1)],
            work_type_mapping: vec![(init_id_1, WorkType::Deal)],
        };
        let actual = toml::to_string(&persistent_state).unwrap();
        let expected = "cores_mapping = [[1, 1], [1, 2], [2, 3], [2, 4], [3, 5], [3, 6], [4, 7], [4, 8]]\n\
        system_cores = [1]\n\
        available_cores = [2, 3]\n\
        unit_id_mapping = [[4, \"54ae1b506c260367a054f80800a545f23e32c6bc4a8908c9a794cb8dad23e5ea\"]]\n\
        work_type_mapping = [[\"54ae1b506c260367a054f80800a545f23e32c6bc4a8908c9a794cb8dad23e5ea\", \"Deal\"]]\n";
        assert_eq!(expected, actual)
    }
}
