use crate::{
    InvestigationEvent, InvestigationFailure, InvestigationRecord, InvestigationTask,
    InvestigationTaskId, ResearchRequest, ResearchState, ResearchStopReason, TaskOrigin,
    TransitionError,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InvestigationTaskStatus {
    Pending,
    Active,
    Completed,
    Failed(InvestigationFailure),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvestigationTaskState {
    task: InvestigationTask,
    status: InvestigationTaskStatus,
}

impl InvestigationTaskState {
    pub const fn task(&self) -> &InvestigationTask {
        &self.task
    }

    pub const fn status(&self) -> &InvestigationTaskStatus {
        &self.status
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InvestigationStatus {
    AwaitingRequest,
    AwaitingPlan,
    Investigating,
    AwaitingNextStep,
    Stopped(ResearchStopReason),
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InvestigationTransitionError {
    #[error("investigation sequence is exhausted")]
    SequenceExhausted,
    #[error("expected investigation record sequence {expected}, found {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("research request is already present")]
    DuplicateRequest,
    #[error("research request is required")]
    RequestRequired,
    #[error("research plan is already present")]
    DuplicatePlan,
    #[error("research plan is required")]
    PlanRequired,
    #[error("investigation task identifier {0} is already present")]
    DuplicateTask(InvestigationTaskId),
    #[error("investigation task identifier {0} is not present")]
    UnknownTask(InvestigationTaskId),
    #[error("investigation task {task_id} is not pending")]
    TaskNotPending {
        task_id: InvestigationTaskId,
        actual: InvestigationTaskStatus,
    },
    #[error("investigation task {task_id} is not active")]
    TaskNotActive {
        task_id: InvestigationTaskId,
        actual: InvestigationTaskStatus,
    },
    #[error("investigation task {0} is not a follow-up")]
    InvalidFollowUpOrigin(InvestigationTaskId),
    #[error("parent investigation task {0} is not completed")]
    ParentTaskNotCompleted(InvestigationTaskId),
    #[error("research is already stopped")]
    ResearchAlreadyStopped,
    #[error("active investigation tasks prevent research from stopping")]
    ActiveTasksPreventStop,
    #[error("investigation result contains an invalid research transition: {0}")]
    ResearchTransition(TransitionError),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InvestigationState {
    last_sequence: u64,
    request: Option<ResearchRequest>,
    plan_recorded: bool,
    tasks: Vec<InvestigationTaskState>,
    stop_reason: Option<ResearchStopReason>,
    research: ResearchState,
}

impl InvestigationState {
    pub fn reconstruct<I>(records: I) -> Result<Self, InvestigationTransitionError>
    where
        I: IntoIterator<Item = InvestigationRecord>,
    {
        let mut state = Self::default();
        for record in records {
            state.apply(record)?;
        }
        Ok(state)
    }

    pub fn apply(
        &mut self,
        record: InvestigationRecord,
    ) -> Result<(), InvestigationTransitionError> {
        let expected = self
            .last_sequence
            .checked_add(1)
            .ok_or(InvestigationTransitionError::SequenceExhausted)?;
        if record.sequence() != expected {
            return Err(InvestigationTransitionError::Sequence {
                expected,
                actual: record.sequence(),
            });
        }
        if self.stop_reason.is_some() {
            return Err(InvestigationTransitionError::ResearchAlreadyStopped);
        }

        let sequence = record.sequence();
        match record.into_event() {
            InvestigationEvent::RequestRecorded(request) => {
                if self.request.is_some() {
                    return Err(InvestigationTransitionError::DuplicateRequest);
                }
                self.request = Some(request);
            }
            InvestigationEvent::PlanRecorded(plan) => {
                if self.request.is_none() {
                    return Err(InvestigationTransitionError::RequestRequired);
                }
                if self.plan_recorded {
                    return Err(InvestigationTransitionError::DuplicatePlan);
                }
                self.tasks = plan
                    .into_tasks()
                    .into_iter()
                    .map(|task| InvestigationTaskState {
                        task,
                        status: InvestigationTaskStatus::Pending,
                    })
                    .collect();
                self.plan_recorded = true;
            }
            InvestigationEvent::TaskStarted { task_id } => {
                self.require_plan()?;
                let index = self.task_index(task_id)?;
                if self.tasks[index].status != InvestigationTaskStatus::Pending {
                    return Err(InvestigationTransitionError::TaskNotPending {
                        task_id,
                        actual: self.tasks[index].status.clone(),
                    });
                }
                self.tasks[index].status = InvestigationTaskStatus::Active;
            }
            InvestigationEvent::TaskCompleted { task_id, result } => {
                self.require_plan()?;
                let index = self.task_index(task_id)?;
                if self.tasks[index].status != InvestigationTaskStatus::Active {
                    return Err(InvestigationTransitionError::TaskNotActive {
                        task_id,
                        actual: self.tasks[index].status.clone(),
                    });
                }
                let mut research = self.research.clone();
                for research_record in result.into_research_records() {
                    research
                        .apply(research_record)
                        .map_err(InvestigationTransitionError::ResearchTransition)?;
                }
                self.research = research;
                self.tasks[index].status = InvestigationTaskStatus::Completed;
            }
            InvestigationEvent::TaskFailed { task_id, failure } => {
                self.require_plan()?;
                let index = self.task_index(task_id)?;
                if self.tasks[index].status != InvestigationTaskStatus::Active {
                    return Err(InvestigationTransitionError::TaskNotActive {
                        task_id,
                        actual: self.tasks[index].status.clone(),
                    });
                }
                self.tasks[index].status = InvestigationTaskStatus::Failed(failure);
            }
            InvestigationEvent::FollowUpRecorded(task) => {
                self.require_plan()?;
                if self
                    .tasks
                    .iter()
                    .any(|candidate| candidate.task.id() == task.id())
                {
                    return Err(InvestigationTransitionError::DuplicateTask(*task.id()));
                }
                let TaskOrigin::FollowUp { parent_task_id, .. } = task.origin() else {
                    return Err(InvestigationTransitionError::InvalidFollowUpOrigin(
                        *task.id(),
                    ));
                };
                let parent_index = self.task_index(*parent_task_id)?;
                if self.tasks[parent_index].status != InvestigationTaskStatus::Completed {
                    return Err(InvestigationTransitionError::ParentTaskNotCompleted(
                        *parent_task_id,
                    ));
                }
                self.tasks.push(InvestigationTaskState {
                    task,
                    status: InvestigationTaskStatus::Pending,
                });
            }
            InvestigationEvent::ResearchStopped(reason) => {
                if self.request.is_none() {
                    return Err(InvestigationTransitionError::RequestRequired);
                }
                if self
                    .tasks
                    .iter()
                    .any(|task| task.status == InvestigationTaskStatus::Active)
                {
                    return Err(InvestigationTransitionError::ActiveTasksPreventStop);
                }
                self.stop_reason = Some(reason);
            }
        }
        self.last_sequence = sequence;
        Ok(())
    }

    fn require_plan(&self) -> Result<(), InvestigationTransitionError> {
        if self.plan_recorded {
            Ok(())
        } else {
            Err(InvestigationTransitionError::PlanRequired)
        }
    }

    fn task_index(
        &self,
        task_id: InvestigationTaskId,
    ) -> Result<usize, InvestigationTransitionError> {
        self.tasks
            .iter()
            .position(|candidate| *candidate.task.id() == task_id)
            .ok_or(InvestigationTransitionError::UnknownTask(task_id))
    }

    pub const fn last_sequence(&self) -> u64 {
        self.last_sequence
    }

    pub const fn request(&self) -> Option<&ResearchRequest> {
        self.request.as_ref()
    }

    pub fn task(&self, id: &InvestigationTaskId) -> Option<&InvestigationTaskState> {
        self.tasks.iter().find(|task| task.task.id() == id)
    }

    pub fn tasks(&self) -> impl Iterator<Item = &InvestigationTaskState> {
        self.tasks.iter()
    }

    pub fn next_pending_task(&self) -> Option<&InvestigationTask> {
        self.tasks
            .iter()
            .find(|task| task.status == InvestigationTaskStatus::Pending)
            .map(InvestigationTaskState::task)
    }

    pub const fn research(&self) -> &ResearchState {
        &self.research
    }

    pub fn status(&self) -> InvestigationStatus {
        if let Some(reason) = &self.stop_reason {
            return InvestigationStatus::Stopped(reason.clone());
        }
        if self.request.is_none() {
            return InvestigationStatus::AwaitingRequest;
        }
        if !self.plan_recorded {
            return InvestigationStatus::AwaitingPlan;
        }
        if self.tasks.iter().any(|task| {
            matches!(
                task.status,
                InvestigationTaskStatus::Pending | InvestigationTaskStatus::Active
            )
        }) {
            return InvestigationStatus::Investigating;
        }
        InvestigationStatus::AwaitingNextStep
    }
}
