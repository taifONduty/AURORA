use std::collections::BTreeSet;

use crate::{InvestigationTaskId, ResearchRecord};

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PlanningValidationError {
    #[error("research question is empty")]
    EmptyResearchQuestion,
    #[error("investigation task objective is empty")]
    EmptyTaskObjective,
    #[error("research gap is empty")]
    EmptyResearchGap,
    #[error("investigation failure is empty")]
    EmptyInvestigationFailure,
    #[error("blocked reason is empty")]
    EmptyBlockedReason,
    #[error("research plan has no tasks")]
    EmptyResearchPlan,
    #[error("research plan repeats task identifier {0}")]
    DuplicatePlanTask(InvestigationTaskId),
    #[error("research plan task {0} is not an initial task")]
    NonInitialPlanTask(InvestigationTaskId),
    #[error("investigation record sequence is zero")]
    ZeroInvestigationSequence,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResearchRequest {
    question: String,
}

impl ResearchRequest {
    pub fn new(question: String) -> Result<Self, PlanningValidationError> {
        if question.trim().is_empty() {
            return Err(PlanningValidationError::EmptyResearchQuestion);
        }
        Ok(Self { question })
    }

    pub fn question(&self) -> &str {
        &self.question
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResearchGap(String);

impl ResearchGap {
    pub fn new(value: String) -> Result<Self, PlanningValidationError> {
        if value.trim().is_empty() {
            return Err(PlanningValidationError::EmptyResearchGap);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskOrigin {
    Initial,
    FollowUp {
        parent_task_id: InvestigationTaskId,
        gap: ResearchGap,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvestigationTask {
    id: InvestigationTaskId,
    objective: String,
    origin: TaskOrigin,
}

impl InvestigationTask {
    pub fn initial(
        id: InvestigationTaskId,
        objective: String,
    ) -> Result<Self, PlanningValidationError> {
        Self::new(id, objective, TaskOrigin::Initial)
    }

    pub fn follow_up(
        id: InvestigationTaskId,
        parent_task_id: InvestigationTaskId,
        objective: String,
        gap: ResearchGap,
    ) -> Result<Self, PlanningValidationError> {
        Self::new(
            id,
            objective,
            TaskOrigin::FollowUp {
                parent_task_id,
                gap,
            },
        )
    }

    fn new(
        id: InvestigationTaskId,
        objective: String,
        origin: TaskOrigin,
    ) -> Result<Self, PlanningValidationError> {
        if objective.trim().is_empty() {
            return Err(PlanningValidationError::EmptyTaskObjective);
        }
        Ok(Self {
            id,
            objective,
            origin,
        })
    }

    pub const fn id(&self) -> &InvestigationTaskId {
        &self.id
    }

    pub fn objective(&self) -> &str {
        &self.objective
    }

    pub const fn origin(&self) -> &TaskOrigin {
        &self.origin
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResearchPlan {
    tasks: Vec<InvestigationTask>,
}

impl ResearchPlan {
    pub fn new(tasks: Vec<InvestigationTask>) -> Result<Self, PlanningValidationError> {
        if tasks.is_empty() {
            return Err(PlanningValidationError::EmptyResearchPlan);
        }
        let mut ids = BTreeSet::new();
        for task in &tasks {
            if !matches!(task.origin(), TaskOrigin::Initial) {
                return Err(PlanningValidationError::NonInitialPlanTask(*task.id()));
            }
            if !ids.insert(*task.id()) {
                return Err(PlanningValidationError::DuplicatePlanTask(*task.id()));
            }
        }
        Ok(Self { tasks })
    }

    pub fn tasks(&self) -> &[InvestigationTask] {
        &self.tasks
    }

    pub(crate) fn into_tasks(self) -> Vec<InvestigationTask> {
        self.tasks
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvestigationResult {
    research_records: Vec<ResearchRecord>,
}

impl InvestigationResult {
    pub fn new(research_records: Vec<ResearchRecord>) -> Self {
        Self { research_records }
    }

    pub fn research_records(&self) -> &[ResearchRecord] {
        &self.research_records
    }

    pub(crate) fn into_research_records(self) -> Vec<ResearchRecord> {
        self.research_records
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvestigationFailure(String);

impl InvestigationFailure {
    pub fn new(value: String) -> Result<Self, PlanningValidationError> {
        if value.trim().is_empty() {
            return Err(PlanningValidationError::EmptyInvestigationFailure);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockedReason(String);

impl BlockedReason {
    pub fn new(value: String) -> Result<Self, PlanningValidationError> {
        if value.trim().is_empty() {
            return Err(PlanningValidationError::EmptyBlockedReason);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResearchStopReason {
    OperatorStopped,
    BudgetExhausted,
    Blocked(BlockedReason),
}
