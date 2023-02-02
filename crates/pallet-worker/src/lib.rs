//! Pallet worker logic.

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

pub mod traits;
pub mod weights;

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use crate::traits::{AtLeast64BitUnsigned, WorkerConstraints};

    use frame_support::pallet_prelude::*;
    use frame_support::traits::Randomness;
    use frame_system::pallet_prelude::*;
    use sp_arithmetic::traits::One;
    use sp_std::vec::Vec;
    use weights::WeightInfo;

    #[pallet::config]
    pub trait Config: frame_system::Config {
        /// Event type.
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;

        /// Task specification type (e.g. docker image and command to run).
        type TaskSpec: Parameter + MaxEncodedLen;
        /// Worker task id type.
        type TaskId: Parameter + MaxEncodedLen + AtLeast64BitUnsigned + Default;
        /// Task TaskResult type.
        type TaskResult: Parameter + MaxEncodedLen;

        /// Worker specification type (e.g. available hardware resources).
        type WorkerSpec: Parameter + MaxEncodedLen;
        /// Worker constraints type.
        type WorkerConstraints: Parameter + MaxEncodedLen + WorkerConstraints<Self::WorkerSpec>;

        /// Randomness generating type.
        type Randomness: Randomness<Self::Hash, Self::BlockNumber>;

        /// The weight information provider type.
        type WeightInfo: WeightInfo;
    }

    #[pallet::pallet]
    #[pallet::generate_store(pub(super) trait Store)]
    pub struct Pallet<T>(_);

    /// The specifications of available workers in the network.
    #[pallet::storage]
    #[pallet::getter(fn workers)]
    pub type Workers<T: Config> =
        StorageMap<_, Twox64Concat, T::AccountId, Worker<T::WorkerSpec>, OptionQuery>;

    #[derive(Clone, Encode, Decode, Eq, PartialEq, RuntimeDebug, MaxEncodedLen, TypeInfo)]
    pub struct Worker<WorkerSpec> {
        spec: WorkerSpec,
        is_online: bool,
    }

    /// Currently assigned task IDs for workers.
    #[pallet::storage]
    #[pallet::getter(fn worker_tasks)]
    pub type WorkerTasks<T: Config> =
        StorageMap<_, Twox64Concat, T::AccountId, T::TaskId, OptionQuery>;

    /// Task IDs mapped to specifications and results.
    #[pallet::storage]
    #[pallet::getter(fn tasks)]
    // FIXME: Should be removed after some time to prevent the storage from growing indefinitely.
    pub type Tasks<T: Config> = StorageMap<
        _,
        Twox64Concat,
        T::TaskId,
        Task<T::TaskSpec, T::WorkerConstraints, T::TaskResult>,
        OptionQuery,
    >;

    #[derive(Clone, Encode, Decode, Eq, PartialEq, RuntimeDebug, MaxEncodedLen, TypeInfo)]
    pub struct Task<TaskSpec, WorkerConstraints, TaskResult> {
        spec: TaskSpec,
        constraints: Option<WorkerConstraints>,
        result: Option<TaskResult>,
    }

    /// Next free task ID.
    #[pallet::storage]
    pub type NextTaskId<T: Config> = StorageValue<_, T::TaskId, ValueQuery>;

    /// Possible events list.
    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// New worker has been registered in the network.
        NewWorker {
            /// The worker id.
            worker_id: T::AccountId,
            /// The worker specification.
            worker_spec: T::WorkerSpec,
        },
        /// Worker has been unregistered.
        WorkerUnregistered {
            /// The worker id.
            worker_id: T::AccountId,
        },
        /// Worker specification has been updated.
        WorkerSpecUpdated {
            /// The worker id.
            worker_id: T::AccountId,
            /// The worker specification.
            worker_spec: T::WorkerSpec,
        },
        /// Worker has gone online (i.e. ready to accept new tasks).
        WorkerOnline {
            /// The worker id.
            worker_id: T::AccountId,
        },
        /// Worker has gone ofline (i.e. not accepting new tasks).
        WorkerOffline {
            /// The worker id.
            worker_id: T::AccountId,
        },
        /// The new task has been assigned for worker.
        RunTask {
            /// Worker id the task has been assigned to.
            worker_id: T::AccountId,
            /// The task id.
            task_id: T::TaskId,
            /// The task that has been assigned.
            task_spec: T::TaskSpec,
            /// Worker constraints for running the task.
            constraints: Option<T::WorkerConstraints>,
        },
        /// The task has been done.
        TaskDone {
            /// The worker of finished task.
            worker_id: T::AccountId,
            /// The task id.
            task_id: T::TaskId,
            /// The task result.
            task_result: T::TaskResult,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        /// Worker already registered.
        WorkerAlreadyRegistered,
        /// No worker was found.
        NoWorkerId,
        /// No submitted task was found.
        NoSubmittedTask,
        /// Task is already done (has a result).
        TaskAlreadyDone,
        /// The worker is busy with a task.
        WorkerIsBusy,
        /// The worker is offline (not accepting tasks).
        WorkerIsOffline,
        /// Cannot find any worker suitable for the task.
        NoSuitableWorkers,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        #[pallet::call_index(0)]
        #[pallet::weight(T::WeightInfo::register())]
        /// Register the worker in the network.
        pub fn register(
            origin: OriginFor<T>,
            spec: T::WorkerSpec,
            is_online: bool,
        ) -> DispatchResult {
            let worker_id = ensure_signed(origin)?;
            ensure!(
                !<Workers<T>>::contains_key(&worker_id),
                Error::<T>::WorkerAlreadyRegistered
            );

            Self::deposit_event(Event::NewWorker {
                worker_id: worker_id.clone(),
                worker_spec: spec.clone(),
            });

            <Workers<T>>::insert(worker_id, Worker { spec, is_online });

            Ok(())
        }

        #[pallet::call_index(1)]
        #[pallet::weight(T::WeightInfo::unregister())]
        /// Unregister the worker.
        pub fn unregister(origin: OriginFor<T>) -> DispatchResult {
            let worker_id = ensure_signed(origin)?;
            ensure!(
                <Workers<T>>::contains_key(&worker_id),
                Error::<T>::NoWorkerId
            );
            ensure!(
                !<WorkerTasks<T>>::contains_key(&worker_id),
                Error::<T>::WorkerIsBusy
            );

            <Workers<T>>::remove(&worker_id);

            Self::deposit_event(Event::WorkerUnregistered { worker_id });

            Ok(())
        }

        #[pallet::call_index(2)]
        #[pallet::weight(T::WeightInfo::update_spec())]
        /// Update worker specification (e.g. available hardware resources).
        pub fn update_spec(origin: OriginFor<T>, spec: T::WorkerSpec) -> DispatchResult {
            let worker_id = ensure_signed(origin)?;
            <Workers<T>>::try_mutate(worker_id.clone(), |worker| {
                let worker = worker.as_mut().ok_or(Error::<T>::NoWorkerId)?;
                worker.spec = spec.clone();
                Ok::<(), Error<T>>(())
            })?;

            Self::deposit_event(Event::WorkerSpecUpdated {
                worker_id,
                worker_spec: spec,
            });

            Ok(())
        }

        #[pallet::call_index(3)]
        #[pallet::weight(T::WeightInfo::go_online())]
        /// Mark the worker as 'online' (i.e. ready to accept tasks).
        pub fn go_online(origin: OriginFor<T>) -> DispatchResult {
            let worker_id = ensure_signed(origin)?;
            <Workers<T>>::try_mutate(worker_id.clone(), |worker| {
                let worker = worker.as_mut().ok_or(Error::<T>::NoWorkerId)?;
                worker.is_online = true;
                Ok::<(), Error<T>>(())
            })?;

            Self::deposit_event(Event::WorkerOnline { worker_id });

            Ok(())
        }

        #[pallet::call_index(4)]
        #[pallet::weight(T::WeightInfo::go_offline())]
        /// Mark the worker as 'offline' (i.e. not accepting new tasks).
        /// NOTE: Marking the worker as 'offline' does **not** mean it can go offline instantly.
        ///       If the worker has a task assigned, it should finish it first.
        pub fn go_offline(origin: OriginFor<T>) -> DispatchResult {
            let worker_id = ensure_signed(origin)?;
            <Workers<T>>::try_mutate(worker_id.clone(), |worker| {
                let worker = worker.as_mut().ok_or(Error::<T>::NoWorkerId)?;
                worker.is_online = false;
                Ok::<(), Error<T>>(())
            })?;

            Self::deposit_event(Event::WorkerOffline { worker_id });

            Ok(())
        }

        #[pallet::call_index(5)]
        #[pallet::weight(T::WeightInfo::done())]
        /// Submit the task result.
        pub fn done(
            origin: OriginFor<T>,
            task_id: T::TaskId,
            task_result: T::TaskResult,
        ) -> DispatchResult {
            let worker_id = ensure_signed(origin)?;

            let current_task_id = <WorkerTasks<T>>::get(&worker_id);
            ensure!(
                matches!(current_task_id, Some(id) if id == task_id),
                Error::<T>::NoSubmittedTask
            );

            <Tasks<T>>::try_mutate(task_id.clone(), |query| {
                let mut task = query
                    .as_mut()
                    .expect("Task ID should never be assigned if no matching task exists");
                ensure!(task.result.is_none(), Error::<T>::TaskAlreadyDone);
                task.result = Some(task_result.clone());
                Ok::<(), Error<T>>(())
            })?;
            <WorkerTasks<T>>::remove(worker_id.clone());

            Self::deposit_event(Event::TaskDone {
                worker_id,
                task_id,
                task_result,
            });

            Ok(())
        }

        #[pallet::call_index(6)]
        #[pallet::weight(T::WeightInfo::submit_task())]
        /// Submit a new task to be computed. Assign to a random matching worker.
        pub fn submit_task(
            origin: OriginFor<T>,
            task_spec: T::TaskSpec,
            constraints: Option<T::WorkerConstraints>,
        ) -> DispatchResult {
            let _submitter = ensure_signed(origin)?; // TODO: Save this?
            let worker_id = Self::select_worker(&constraints)?;
            Self::run_task(worker_id, task_spec, constraints)
        }

        #[pallet::call_index(7)]
        #[pallet::weight(T::WeightInfo::force_run_task())]
        /// Submit a new task to be computed. Assign to a specific worker.
        /// Only callable via root origin.
        pub fn force_run_task(
            origin: OriginFor<T>,
            worker_id: T::AccountId,
            task_spec: T::TaskSpec,
            constraints: Option<T::WorkerConstraints>,
        ) -> DispatchResult {
            ensure_root(origin)?;
            let worker = <Workers<T>>::get(&worker_id).ok_or(Error::<T>::NoWorkerId)?;
            ensure!(worker.is_online, Error::<T>::WorkerIsOffline);
            Self::run_task(worker_id, task_spec, constraints)
        }
    }

    impl<T: Config> Pallet<T> {
        /// Randomly pick a suitable, free worker.
        pub fn select_worker(
            constraints: &Option<T::WorkerConstraints>,
        ) -> Result<T::AccountId, DispatchError> {
            let suitable_workers: Vec<T::AccountId> = <Workers<T>>::iter()
                .filter_map(|(worker_id, worker)| {
                    if !worker.is_online {
                        return None;
                    }
                    match constraints {
                        None => Some(worker_id),
                        Some(c) if c.worker_suitable(&worker.spec) => Some(worker_id),
                        _ => None,
                    }
                })
                .collect();

            ensure!(!suitable_workers.is_empty(), Error::<T>::NoSuitableWorkers);

            // We're using task_id as nonce for randomness
            let (random_hash, _) =
                T::Randomness::random(<NextTaskId<T>>::get().encode().as_slice());
            let random_int = <u32>::decode(&mut random_hash.as_ref())
                .expect("Secure hash should always be bigger than u32")
                as usize;
            let worker = suitable_workers[random_int % suitable_workers.len()].clone();

            Ok(worker)
        }

        /// Assign the task to particular worker.
        pub fn run_task(
            worker_id: T::AccountId,
            task_spec: T::TaskSpec,
            constraints: Option<T::WorkerConstraints>,
        ) -> DispatchResult {
            ensure!(
                !<WorkerTasks<T>>::contains_key(worker_id.clone()),
                Error::<T>::WorkerIsBusy
            );

            let task_id = <NextTaskId<T>>::get();
            <NextTaskId<T>>::mutate(|id| *id += T::TaskId::one()); // u64 is big enough for 1B tasks per second for 100 years

            <Tasks<T>>::insert(
                task_id.clone(),
                Task {
                    spec: task_spec.clone(),
                    constraints: constraints.clone(),
                    result: None,
                },
            );
            <WorkerTasks<T>>::insert(worker_id.clone(), task_id.clone());

            Self::deposit_event(Event::RunTask {
                worker_id,
                task_id,
                task_spec,
                constraints,
            });

            Ok(())
        }
    }
}
