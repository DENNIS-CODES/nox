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

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::ops::Deref;
use std::path::PathBuf;

use ccp_shared::types::{LogicalCoreId, PhysicalCoreId, CUID};
use cpu_utils::CPUTopology;
use fxhash::FxBuildHasher;
use parking_lot::RwLock;
use range_set_blaze::RangeSetBlaze;

use crate::errors::{AcquireError, CreateError, LoadingError, PersistError};
use crate::manager::CoreManagerFunctions;
use crate::persistence::{
    PersistenceTask, PersistentCoreManagerFunctions, PersistentCoreManagerState,
};
use crate::types::{AcquireRequest, Assignment, Cores, WorkType};
use crate::{CoreRange, Map, MultiMap};

/// `DevCoreManager` is a CPU core manager that provides a more flexible approach to
/// core allocation compared to `StrictCoreManager`.
/// It allows for dynamic assignment and release of CPU cores based on workload requirements.
/// While it maintains core allocation constraints, it offers more leniency in core distribution,
/// making it suitable for scenarios where workload priorities may vary and strict allocation
/// policies are not necessary.
pub struct DevCoreManager {
    // path to the persistent state
    file_path: PathBuf,
    // inner state
    state: RwLock<CoreManagerState>,
    // persistent task notification channel
    sender: tokio::sync::mpsc::Sender<()>,
}

impl DevCoreManager {
    /// Loads the state from `file_name` if exists. If not creates a new empty state
    pub fn from_path(
        file_path: PathBuf,
        system_cpu_count: usize,
        core_range: CoreRange,
    ) -> Result<(Self, PersistenceTask), LoadingError> {
        let exists = file_path.exists();
        if exists {
            let bytes = std::fs::read(&file_path).map_err(|err| LoadingError::IoError { err })?;
            let raw_str = std::str::from_utf8(bytes.as_slice())
                .map_err(|err| LoadingError::DecodeError { err })?;
            let persistent_state: PersistentCoreManagerState = toml::from_str(raw_str)
                .map_err(|err| LoadingError::DeserializationError { err })?;

            let config_range = core_range.clone().0;
            let mut loaded_range = RangeSetBlaze::new();
            for (physical_core_id, _) in persistent_state.cores_mapping.clone() {
                loaded_range.insert(<PhysicalCoreId as Into<u32>>::into(physical_core_id) as usize);
            }

            if config_range == loaded_range
                && persistent_state.system_cores.len() == system_cpu_count
            {
                let state: CoreManagerState = persistent_state.into();
                Ok(Self::make_instance_with_task(file_path, state))
            } else {
                tracing::warn!(target: "core-manager", "The initial config has been changed. Ignoring persisted core mapping");
                let (core_manager, task) =
                    Self::new(file_path.clone(), system_cpu_count, core_range)
                        .map_err(|err| LoadingError::CreateCoreManager { err })?;
                core_manager
                    .persist()
                    .map_err(|err| LoadingError::PersistError { err })?;
                Ok((core_manager, task))
            }
        } else {
            tracing::debug!(target: "core-manager", "No persisted core mapping was not found. Creating a new one");
            let (core_manager, task) = Self::new(file_path.clone(), system_cpu_count, core_range)
                .map_err(|err| LoadingError::CreateCoreManager { err })?;
            core_manager
                .persist()
                .map_err(|err| LoadingError::PersistError { err })?;
            Ok((core_manager, task))
        }
    }

    /// Creates an empty core manager with only system cores assigned
    fn new(
        file_name: PathBuf,
        system_cpu_count: usize,
        core_range: CoreRange,
    ) -> Result<(Self, PersistenceTask), CreateError> {
        let available_core_count = core_range.0.len() as usize;

        if system_cpu_count == 0 {
            return Err(CreateError::IllegalSystemCoreCount);
        }

        if system_cpu_count > available_core_count {
            return Err(CreateError::NotEnoughCores {
                available: available_core_count,
                required: system_cpu_count,
            });
        }

        // to observe CPU topology
        let topology = CPUTopology::new().map_err(|err| CreateError::CreateTopology { err })?;

        // retrieve info about physical cores
        let physical_cores = topology
            .physical_cores()
            .map_err(|err| CreateError::CollectCoresData { err })?;

        if !core_range.is_subset(&physical_cores) {
            return Err(CreateError::WrongCpuRange);
        }

        let mut cores_mapping: MultiMap<PhysicalCoreId, LogicalCoreId> =
            MultiMap::with_capacity_and_hasher(available_core_count, FxBuildHasher::default());

        let mut available_cores: BTreeSet<PhysicalCoreId> = BTreeSet::new();

        for physical_core_id in physical_cores {
            if core_range
                .0
                .contains(<PhysicalCoreId as Into<u32>>::into(physical_core_id) as usize)
            {
                let logical_cores = topology
                    .logical_cores_for_physical(physical_core_id)
                    .map_err(|err| CreateError::CollectCoresData { err })?;
                available_cores.insert(physical_core_id);
                for logical_core_id in logical_cores {
                    cores_mapping.insert(physical_core_id, logical_core_id)
                }
            }
        }

        let mut system_cores: BTreeSet<PhysicalCoreId> = BTreeSet::new();
        for _ in 0..system_cpu_count {
            // SAFETY: this should never happen because we already checked the availability of cores
            system_cores.insert(
                available_cores
                    .pop_first()
                    .expect("Unexpected state. Should not be empty never"),
            );
        }

        let core_unit_id_mapping = MultiMap::with_hasher(FxBuildHasher::default());

        let unit_id_core_mapping = Map::with_hasher(FxBuildHasher::default());

        let type_mapping =
            Map::with_capacity_and_hasher(available_core_count, FxBuildHasher::default());

        let available_cores = available_cores.into_iter().collect();

        let inner_state = CoreManagerState {
            cores_mapping,
            system_cores,
            available_cores,
            core_unit_id_mapping,
            unit_id_core_mapping,
            work_type_mapping: type_mapping,
        };

        let result = Self::make_instance_with_task(file_name, inner_state);

        Ok(result)
    }

    fn make_instance_with_task(
        file_name: PathBuf,
        state: CoreManagerState,
    ) -> (Self, PersistenceTask) {
        // This channel is used to notify a persistent task about changes.
        // It has a size of 1 because we need only the fact that this change happen
        let (sender, receiver) = tokio::sync::mpsc::channel(1);

        (
            Self {
                file_path: file_name,
                sender,
                state: RwLock::new(state),
            },
            PersistenceTask::new(receiver),
        )
    }
}

struct CoreManagerState {
    // mapping between physical and logical cores
    cores_mapping: MultiMap<PhysicalCoreId, LogicalCoreId>,
    // allocated system cores
    system_cores: BTreeSet<PhysicalCoreId>,
    // free physical cores
    available_cores: VecDeque<PhysicalCoreId>,
    // mapping between physical core id and unit id
    core_unit_id_mapping: MultiMap<PhysicalCoreId, CUID>,

    unit_id_core_mapping: Map<CUID, PhysicalCoreId>,
    // mapping between unit id and workload type
    work_type_mapping: Map<CUID, WorkType>,
}

impl From<&CoreManagerState> for PersistentCoreManagerState {
    fn from(value: &CoreManagerState) -> Self {
        Self {
            cores_mapping: value.cores_mapping.iter().map(|(k, v)| (*k, *v)).collect(),
            system_cores: value.system_cores.iter().cloned().collect(),
            available_cores: value.available_cores.iter().cloned().collect(),
            unit_id_mapping: value
                .core_unit_id_mapping
                .iter()
                .map(|(k, v)| (*k, (*v)))
                .collect(),
            work_type_mapping: value
                .work_type_mapping
                .iter()
                .map(|(k, v)| ((*k), v.clone()))
                .collect(),
        }
    }
}

impl From<PersistentCoreManagerState> for CoreManagerState {
    fn from(value: PersistentCoreManagerState) -> Self {
        Self {
            cores_mapping: value.cores_mapping.into_iter().collect(),
            system_cores: value.system_cores.into_iter().collect(),
            available_cores: value.available_cores.into_iter().collect(),
            core_unit_id_mapping: value.unit_id_mapping.iter().cloned().collect(),
            unit_id_core_mapping: value
                .unit_id_mapping
                .into_iter()
                .map(|(core_id, unit_id)| (unit_id, core_id))
                .collect(),
            work_type_mapping: value.work_type_mapping.into_iter().collect(),
        }
    }
}

impl CoreManagerFunctions for DevCoreManager {
    fn acquire_worker_core(
        &self,
        assign_request: AcquireRequest,
    ) -> Result<Assignment, AcquireError> {
        let mut lock = self.state.write();
        let mut result_physical_core_ids = BTreeSet::new();
        let mut result_logical_core_ids = BTreeSet::new();
        let mut cuid_cores: Map<CUID, Cores> = HashMap::with_capacity_and_hasher(
            assign_request.unit_ids.len(),
            FxBuildHasher::default(),
        );
        let worker_unit_type = assign_request.worker_type;
        for unit_id in assign_request.unit_ids {
            let physical_core_id = lock.unit_id_core_mapping.get(&unit_id).cloned();
            let physical_core_id = match physical_core_id {
                None => {
                    // SAFETY: this should never happen because after the pop operation, we push it back
                    let core_id = lock
                        .available_cores
                        .pop_front()
                        .expect("Unexpected state. Should not be empty never");
                    lock.core_unit_id_mapping.insert(core_id, unit_id);
                    lock.unit_id_core_mapping.insert(unit_id, core_id);
                    lock.work_type_mapping
                        .insert(unit_id, worker_unit_type.clone());
                    lock.available_cores.push_back(core_id);
                    core_id
                }
                Some(core_id) => {
                    lock.work_type_mapping
                        .insert(unit_id, worker_unit_type.clone());
                    core_id
                }
            };
            result_physical_core_ids.insert(physical_core_id);

            // SAFETY: The physical core always has corresponding logical ids,
            // unit_id_core_mapping can't have a wrong physical_core_id
            let logical_core_ids = lock
                .cores_mapping
                .get_vec(&physical_core_id)
                .cloned()
                .expect("Unexpected state. Should not be empty never");

            for logical_core in logical_core_ids.iter() {
                result_logical_core_ids.insert(*logical_core);
            }

            cuid_cores.insert(
                unit_id,
                Cores {
                    physical_core_id,
                    logical_core_ids,
                },
            );
        }

        // We are trying to notify a persistence task that the state has been changed.
        // We don't care if the channel is full, it means the current state will be stored with the previous event
        let _ = self.sender.try_send(());

        Ok(Assignment {
            physical_core_ids: result_physical_core_ids,
            logical_core_ids: result_logical_core_ids,
            cuid_cores,
        })
    }

    fn release(&self, unit_ids: &[CUID]) {
        let mut lock = self.state.write();
        for unit_id in unit_ids {
            if let Some(physical_core_id) = lock.unit_id_core_mapping.remove(unit_id) {
                let mapping = lock.core_unit_id_mapping.get_vec_mut(&physical_core_id);
                if let Some(mapping) = mapping {
                    let index = mapping.iter().position(|x| x == unit_id).unwrap();
                    mapping.remove(index);
                    if mapping.is_empty() {
                        lock.core_unit_id_mapping.remove(&physical_core_id);
                    }
                }
                lock.work_type_mapping.remove(unit_id);
            }
        }
    }

    fn get_system_cpu_assignment(&self) -> Assignment {
        let lock = self.state.read();
        let mut logical_core_ids = BTreeSet::new();
        for core in &lock.system_cores {
            // SAFETY: The physical core always has corresponding logical ids,
            // system cores can't have a wrong physical_core_id
            let core_ids = lock
                .cores_mapping
                .get_vec(core)
                .cloned()
                .expect("Unexpected state. Should not be empty never");
            for core_id in core_ids {
                logical_core_ids.insert(core_id);
            }
        }
        Assignment {
            physical_core_ids: lock.system_cores.clone(),
            logical_core_ids,
            cuid_cores: Map::with_hasher(FxBuildHasher::default()),
        }
    }
}

impl PersistentCoreManagerFunctions for DevCoreManager {
    fn persist(&self) -> Result<(), PersistError> {
        let lock = self.state.read();
        let inner_state = lock.deref();
        let persistent_state: PersistentCoreManagerState = inner_state.into();
        drop(lock);
        persistent_state.persist(self.file_path.as_path())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use ccp_shared::types::CUID;
    use hex::FromHex;
    use rand::RngCore;
    use std::str::FromStr;

    use crate::manager::CoreManagerFunctions;
    use crate::types::{AcquireRequest, WorkType};
    use crate::{CoreRange, DevCoreManager, StrictCoreManager};

    fn cores_exists() -> bool {
        num_cpus::get_physical() >= 4
    }

    #[test]
    fn test_acquire_and_switch() {
        if cores_exists() {
            let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

            let (manager, _task) = DevCoreManager::from_path(
                temp_dir.path().join("test.toml"),
                2,
                CoreRange::default(),
            )
            .unwrap();
            let init_id_1 = <CUID>::from_hex(
                "54ae1b506c260367a054f80800a545f23e32c6bc4a8908c9a794cb8dad23e5ea",
            )
            .unwrap();
            let init_id_2 = <CUID>::from_hex(
                "1cce3d08f784b11d636f2fb55adf291d43c2e9cbe7ae7eeb2d0301a96be0a3a0",
            )
            .unwrap();
            let unit_ids = vec![init_id_1, init_id_2];
            let assignment_1 = manager
                .acquire_worker_core(AcquireRequest {
                    unit_ids: unit_ids.clone(),
                    worker_type: WorkType::CapacityCommitment,
                })
                .unwrap();
            let assignment_2 = manager
                .acquire_worker_core(AcquireRequest {
                    unit_ids: unit_ids.clone(),
                    worker_type: WorkType::Deal,
                })
                .unwrap();
            let assignment_3 = manager
                .acquire_worker_core(AcquireRequest {
                    unit_ids: unit_ids.clone(),
                    worker_type: WorkType::CapacityCommitment,
                })
                .unwrap();
            assert_eq!(assignment_1, assignment_2);
            assert_eq!(assignment_1, assignment_3);
        }
    }

    #[test]
    fn test_acquire_and_release() {
        if cores_exists() {
            let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
            let system_cpu_count = 2;
            let (manager, _task) = DevCoreManager::from_path(
                temp_dir.path().join("test.toml"),
                system_cpu_count,
                CoreRange::default(),
            )
            .unwrap();
            let before_lock = manager.state.read();

            let before_available_core = before_lock.available_cores.clone();
            let before_unit_id_mapping = before_lock.core_unit_id_mapping.clone();
            let before_type_mapping = before_lock.work_type_mapping.clone();
            drop(before_lock);

            assert_eq!(
                before_available_core.len(),
                num_cpus::get_physical() - system_cpu_count
            );
            assert_eq!(before_unit_id_mapping.len(), 0);
            assert_eq!(before_type_mapping.len(), 0);

            let init_id_1 = <CUID>::from_hex(
                "54ae1b506c260367a054f80800a545f23e32c6bc4a8908c9a794cb8dad23e5ea",
            )
            .unwrap();
            let init_id_2 = <CUID>::from_hex(
                "1cce3d08f784b11d636f2fb55adf291d43c2e9cbe7ae7eeb2d0301a96be0a3a0",
            )
            .unwrap();
            let unit_ids = vec![init_id_1, init_id_2];
            let assignment = manager
                .acquire_worker_core(AcquireRequest {
                    unit_ids: unit_ids.clone(),
                    worker_type: WorkType::CapacityCommitment,
                })
                .unwrap();
            assert_eq!(assignment.physical_core_ids.len(), 2);

            let after_assignment = manager.state.read();

            let after_assignment_available_core = after_assignment.available_cores.clone();
            let after_assignment_unit_id_mapping = after_assignment.core_unit_id_mapping.clone();
            let after_assignment_type_mapping = after_assignment.work_type_mapping.clone();
            drop(after_assignment);

            assert_eq!(
                after_assignment_available_core.len(),
                num_cpus::get_physical() - system_cpu_count
            );
            assert_eq!(after_assignment_unit_id_mapping.len(), 2);
            assert_eq!(after_assignment_type_mapping.len(), 2);

            manager.release(&unit_ids);

            let after_release_lock = manager.state.read();

            let after_release_unit_id_mapping = after_release_lock.core_unit_id_mapping.clone();
            let after_release_type_mapping = after_release_lock.work_type_mapping.clone();
            drop(after_release_lock);

            assert_eq!(after_release_unit_id_mapping, before_unit_id_mapping);
            assert_eq!(after_release_type_mapping, before_type_mapping);
        }
    }

    #[test]
    fn test_oversell_acquire() {
        if cores_exists() {
            let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
            let system_cpu_count = 2;
            let (manager, _task) = DevCoreManager::from_path(
                temp_dir.path().join("test.toml"),
                system_cpu_count,
                CoreRange::default(),
            )
            .unwrap();
            let before_lock = manager.state.read();

            let before_available_core = before_lock.available_cores.clone();
            let before_unit_id_mapping = before_lock.core_unit_id_mapping.clone();
            let before_type_mapping = before_lock.work_type_mapping.clone();
            drop(before_lock);

            assert_eq!(
                before_available_core.len(),
                num_cpus::get_physical() - system_cpu_count
            );
            assert_eq!(before_unit_id_mapping.len(), 0);
            assert_eq!(before_type_mapping.len(), 0);

            let assignment_count = before_available_core.len() * 2;

            for _ in 0..assignment_count {
                let mut bytes = [0; 32];

                rand::thread_rng().fill_bytes(&mut bytes);
                let init_id_1 = <CUID>::from_hex(hex::encode(bytes)).unwrap();

                rand::thread_rng().fill_bytes(&mut bytes);
                let init_id_2 = <CUID>::from_hex(hex::encode(bytes)).unwrap();

                let unit_ids = vec![init_id_1, init_id_2];
                let assignment = manager
                    .acquire_worker_core(AcquireRequest {
                        unit_ids: unit_ids.clone(),
                        worker_type: WorkType::Deal,
                    })
                    .unwrap();
                assert_eq!(assignment.physical_core_ids.len(), 2);
            }

            let after_assignment = manager.state.read();

            let after_assignment_available_core = after_assignment.available_cores.clone();
            let after_assignment_unit_id_mapping = after_assignment.core_unit_id_mapping.clone();
            let after_assignment_type_mapping = after_assignment.work_type_mapping.clone();
            drop(after_assignment);

            assert_eq!(
                after_assignment_available_core.len(),
                num_cpus::get_physical() - system_cpu_count
            );
            assert_eq!(
                after_assignment_unit_id_mapping.len(),
                before_available_core.len()
            );
            assert_eq!(after_assignment_type_mapping.len(), assignment_count * 2);
        }
    }

    #[test]
    fn test_wrong_range() {
        if cores_exists() {
            let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

            let range = CoreRange::from_str("0-16384").unwrap();

            let result = StrictCoreManager::from_path(temp_dir.path().join("test.toml"), 2, range);

            assert!(result.is_err());
            assert_eq!(
                result.err().map(|err| err.to_string()),
                Some("The specified CPU range exceeds the available CPU count".to_string())
            );
        }
    }
}
