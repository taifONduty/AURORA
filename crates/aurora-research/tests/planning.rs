use aurora_research::{
    BlockedReason, InvestigationFailure, InvestigationResult, InvestigationTask,
    InvestigationTaskId, PlanningValidationError, ResearchGap, ResearchPlan, ResearchRequest,
    ResearchStopReason, TaskOrigin,
};

#[test]
fn request_plan_and_follow_up_preserve_domain_input() {
    let request =
        ResearchRequest::new("Why is the sky blue?".to_owned()).expect("question is valid");
    let initial = InvestigationTask::initial(
        task_id(1),
        "Find the primary physical explanation".to_owned(),
    )
    .expect("initial task is valid");
    let plan = ResearchPlan::new(vec![initial.clone()]).expect("plan is valid");
    let gap = ResearchGap::new("The result omitted wavelength dependence".to_owned())
        .expect("gap is valid");
    let follow_up = InvestigationTask::follow_up(
        task_id(2),
        *initial.id(),
        "Resolve the wavelength dependence".to_owned(),
        gap,
    )
    .expect("follow-up is valid");

    assert_eq!(request.question(), "Why is the sky blue?");
    assert_eq!(plan.tasks(), &[initial]);
    assert_eq!(follow_up.objective(), "Resolve the wavelength dependence");
    assert!(matches!(
        follow_up.origin(),
        TaskOrigin::FollowUp {
            parent_task_id,
            gap
        } if *parent_task_id == task_id(1)
            && gap.as_str() == "The result omitted wavelength dependence"
    ));
}

#[test]
fn blank_planning_text_is_rejected() {
    assert_eq!(
        ResearchRequest::new(" \n".to_owned()),
        Err(PlanningValidationError::EmptyResearchQuestion)
    );
    assert_eq!(
        InvestigationTask::initial(task_id(1), "\t".to_owned()),
        Err(PlanningValidationError::EmptyTaskObjective)
    );
    assert_eq!(
        ResearchGap::new(" ".to_owned()),
        Err(PlanningValidationError::EmptyResearchGap)
    );
    assert_eq!(
        InvestigationFailure::new(" ".to_owned()),
        Err(PlanningValidationError::EmptyInvestigationFailure)
    );
    assert_eq!(
        BlockedReason::new(" ".to_owned()),
        Err(PlanningValidationError::EmptyBlockedReason)
    );
}

#[test]
fn plan_requires_distinct_initial_tasks() {
    assert_eq!(
        ResearchPlan::new(Vec::new()),
        Err(PlanningValidationError::EmptyResearchPlan)
    );

    let task = InvestigationTask::initial(task_id(1), "First objective".to_owned())
        .expect("task is valid");
    assert_eq!(
        ResearchPlan::new(vec![task.clone(), task]),
        Err(PlanningValidationError::DuplicatePlanTask(task_id(1)))
    );

    let follow_up = InvestigationTask::follow_up(
        task_id(2),
        task_id(1),
        "Follow-up objective".to_owned(),
        ResearchGap::new("Unresolved gap".to_owned()).expect("gap is valid"),
    )
    .expect("follow-up is valid");
    assert_eq!(
        ResearchPlan::new(vec![follow_up]),
        Err(PlanningValidationError::NonInitialPlanTask(task_id(2)))
    );
}

#[test]
fn result_may_contain_no_research_records() {
    let result = InvestigationResult::new(Vec::new());

    assert!(result.research_records().is_empty());
}

#[test]
fn failure_and_stop_reasons_remain_distinct() {
    let failure = InvestigationFailure::new("Search endpoint unavailable".to_owned())
        .expect("failure is valid");
    let blocked = BlockedReason::new("Required archive cannot be accessed".to_owned())
        .expect("blocked reason is valid");

    assert_eq!(failure.as_str(), "Search endpoint unavailable");
    assert_eq!(blocked.as_str(), "Required archive cannot be accessed");
    assert_ne!(
        ResearchStopReason::OperatorStopped,
        ResearchStopReason::BudgetExhausted
    );
    assert_eq!(
        ResearchStopReason::Blocked(blocked.clone()),
        ResearchStopReason::Blocked(blocked)
    );
}

fn task_id(value: u128) -> InvestigationTaskId {
    let mut bytes = value.to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes)
        .hyphenated()
        .to_string()
        .parse()
        .expect("fixture identifier is valid")
}
