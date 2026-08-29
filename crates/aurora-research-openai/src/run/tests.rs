use std::{
    collections::VecDeque,
    future::{Future, pending, ready},
    pin::Pin,
};

use aurora_core::ModelRequestFailure;
use aurora_openai::StructuredOutputInvocation;
use aurora_research::{
    ContentDigest, Evidence, EvidenceId, InvestigationEvent, InvestigationResult,
    InvestigationTask, InvestigationTaskStatus, MediaType, ResearchControlEvent,
    ResearchControlLimits, ResearchControlState, ResearchControlStatus, ResearchEvent,
    ResearchGapCause, ResearchGapStatus, ResearchRecord, ResearchRequest, RetrievedAt, Source,
    SourceId, TaskOrigin,
};
use aurora_tavily::TavilyFailure;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{
    ModelDrivenResearchIssue, ModelProposalRequest, ProposalFuture, ProposalModel,
    ResearchRetrieval, RetrievalFuture, SequentialRun, execute,
};
use crate::context::MAX_MODEL_CONTEXT_BYTES;

#[tokio::test]
async fn oversized_initial_question_stops_before_model_invocation_without_truncation() {
    let question = "q".repeat(MAX_MODEL_CONTEXT_BYTES);
    let request = ResearchRequest::new(question.clone()).unwrap();
    let mut model = ScriptedModel::new([]);
    let mut retrieval = ScriptedRetrieval::new([]);

    let run = execute(
        &mut model,
        &mut retrieval,
        request,
        ResearchControlLimits::new(0),
        retrieved_at(),
        CancellationToken::new(),
    )
    .await
    .expect("oversized model input is a domain-terminal result");

    assert_eq!(
        run.issue(),
        Some(&ModelDrivenResearchIssue::ModelInputTooLarge)
    );
    assert!(model.calls.is_empty());
    assert!(retrieval.queries.is_empty());
    assert_eq!(
        run.state().investigation().request().unwrap().question(),
        question
    );
    assert_noncompleted_reconstructs(&run);
}

#[tokio::test]
async fn cancellation_precedes_an_oversized_initial_model_context() {
    let request = ResearchRequest::new("q".repeat(MAX_MODEL_CONTEXT_BYTES)).unwrap();
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let mut model = ScriptedModel::new([]);
    let mut retrieval = ScriptedRetrieval::new([]);

    let run = execute(
        &mut model,
        &mut retrieval,
        request,
        ResearchControlLimits::new(0),
        retrieved_at(),
        cancellation,
    )
    .await
    .expect("cancellation remains a domain-terminal result");

    assert_eq!(run.issue(), Some(&ModelDrivenResearchIssue::Cancelled));
    assert!(model.calls.is_empty());
    assert!(retrieval.queries.is_empty());
    assert_noncompleted_reconstructs(&run);
}

#[tokio::test]
async fn extraction_context_escaping_overflow_fails_task_without_partial_admission() {
    let mut escaped_snapshot = String::with_capacity(1024 * 1024);
    escaped_snapshot.push('x');
    escaped_snapshot.extend(std::iter::repeat_n('\n', 1024 * 1024 - 1));
    let mut model = ScriptedModel::new([output(
        json!({"tasks":[{"objective":"escaped snapshot query"}]}),
    )]);
    let mut retrieval = ScriptedRetrieval::new([Ok(acquired(&escaped_snapshot))]);

    let run = execute(
        &mut model,
        &mut retrieval,
        request(),
        ResearchControlLimits::new(0),
        retrieved_at(),
        CancellationToken::new(),
    )
    .await
    .expect("oversized extraction context is inspectable");

    assert_eq!(
        run.issue(),
        Some(&ModelDrivenResearchIssue::ModelInputTooLarge)
    );
    assert_eq!(model.calls.len(), 1);
    assert!(matches!(
        run.state().investigation().tasks().next().unwrap().status(),
        InvestigationTaskStatus::Failed(_)
    ));
    assert_eq!(run.state().investigation().research().source_count(), 0);
    assert_noncompleted_reconstructs(&run);
}

#[tokio::test]
async fn accumulated_verification_context_stops_after_complete_task_admission() {
    let first = format!("Exact acquired fact{}", "a".repeat(1024 * 1024 - 19));
    let second = format!("Follow-up fact{}", "b".repeat(1024 * 1024 - 14));
    let mut model = ScriptedModel::new([
        output(json!({"tasks":[{"objective":"initial query"}]})),
        grounded_extraction(),
        insufficient_verification(),
        output(json!({"objective":"follow-up query"})),
        follow_up_extraction("Follow-up fact"),
    ]);
    let mut retrieval = ScriptedRetrieval::new([Ok(acquired(&first)), Ok(acquired_at(&second, 5))]);

    let run = execute(
        &mut model,
        &mut retrieval,
        request(),
        ResearchControlLimits::new(1),
        retrieved_at(),
        CancellationToken::new(),
    )
    .await
    .expect("cumulative model input overflow is inspectable");

    assert_eq!(
        run.issue(),
        Some(&ModelDrivenResearchIssue::ModelInputTooLarge)
    );
    assert_eq!(model.calls.len(), 5);
    assert_eq!(run.state().investigation().research().source_count(), 2);
    assert!(
        run.state()
            .investigation()
            .tasks()
            .all(|task| matches!(task.status(), InvestigationTaskStatus::Completed))
    );
    assert_noncompleted_reconstructs(&run);
}

#[tokio::test]
async fn accumulated_follow_up_context_stops_before_partial_task_admission() {
    let large_objective = "query".repeat(30 * 1024);
    let first = format!("Exact acquired fact{}", "a".repeat(900 * 1024));
    let second = format!("Follow-up fact{}", "b".repeat(900 * 1024));
    let mut model = ScriptedModel::new([
        output(json!({"tasks":[{"objective":large_objective}]})),
        grounded_extraction(),
        insufficient_verification(),
        output(json!({"objective":large_objective})),
        follow_up_extraction("Follow-up fact"),
        insufficient_verification(),
    ]);
    let mut retrieval = ScriptedRetrieval::new([Ok(acquired(&first)), Ok(acquired_at(&second, 5))]);

    let run = execute(
        &mut model,
        &mut retrieval,
        request(),
        ResearchControlLimits::new(2),
        retrieved_at(),
        CancellationToken::new(),
    )
    .await
    .expect("cumulative follow-up context overflow is inspectable");

    assert_eq!(
        run.issue(),
        Some(&ModelDrivenResearchIssue::ModelInputTooLarge)
    );
    assert_eq!(model.calls.len(), 6);
    assert_eq!(run.state().follow_up_count(), 1);
    assert_eq!(run.state().investigation().research().source_count(), 2);
    assert_noncompleted_reconstructs(&run);
}

#[tokio::test]
async fn initial_model_driven_path_records_grounded_verified_completion() {
    let mut model = ScriptedModel::new([
        output(json!({"tasks":[{"objective":"exact Tavily query"}]})),
        output(json!({
            "evidence":[{"source_index":0,"excerpt":"Exact acquired fact"}],
            "claims":[{"statement":"The fact is grounded","evidence_indices":[0]}]
        })),
        output(json!({
            "relations":[{"evidence_index":0,"relation":"supports"}],
            "sufficiency":"sufficient"
        })),
    ]);
    let mut retrieval = ScriptedRetrieval::new([Ok(acquired("Exact acquired fact in context"))]);

    let run = execute(
        &mut model,
        &mut retrieval,
        request(),
        ResearchControlLimits::new(0),
        retrieved_at(),
        CancellationToken::new(),
    )
    .await
    .expect("generated records remain valid");

    assert_eq!(run.issue(), None);
    assert_eq!(run.state().status(), ResearchControlStatus::Completed);
    assert_eq!(retrieval.queries, ["exact Tavily query"]);
    assert_eq!(model.calls.len(), 3);
    assert_eq!(model.calls[0].name, "initial_plan");
    assert_eq!(model.calls[1].name, "evidence_extraction");
    assert_eq!(model.calls[2].name, "claim_verification");

    let planning: Value = serde_json::from_str(&model.calls[0].input).unwrap();
    assert_eq!(planning.as_object().unwrap().len(), 1);
    assert_eq!(planning["research_question"], "What is the fact?");
    let verification: Value = serde_json::from_str(&model.calls[2].input).unwrap();
    assert_eq!(verification.as_object().unwrap().len(), 3);
    assert_eq!(verification["research_question"], "What is the fact?");
    assert_eq!(verification["claim_statement"], "The fact is grounded");
    assert!(
        verification["evidence"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );
    assert!(verification.get("generator_context").is_none());
    assert!(verification.get("response_id").is_none());
    assert!(verification.get("plan").is_none());

    let state = run.state();
    let task = state
        .investigation()
        .tasks()
        .next()
        .expect("one generated task");
    assert_eq!(task.task().objective(), "exact Tavily query");
    assert!(matches!(task.status(), InvestigationTaskStatus::Completed));
    let research = state.investigation().research();
    assert_eq!(research.source_count(), 1);
    assert_eq!(research.evidence_count(), 2);
    assert_eq!(research.claim_count(), 1);
    let claim = research.claims().next().unwrap();
    assert_eq!(claim.statement(), "The fact is grounded");
    assert_eq!(claim.evidence_ids().len(), 1);
    assert!(
        claim
            .evidence_ids()
            .iter()
            .all(|id| research.evidence(id).is_some())
    );
    assert_eq!(state.verification().assessments().count(), 1);

    assert!(matches!(
        run.records()[0].event(),
        ResearchControlEvent::LimitsRecorded(_)
    ));
    assert!(
        matches!(run.records()[1].event(), ResearchControlEvent::InvestigationAdvanced(record) if matches!(record.event(), InvestigationEvent::RequestRecorded(_)))
    );
    assert!(
        matches!(run.records()[2].event(), ResearchControlEvent::InvestigationAdvanced(record) if matches!(record.event(), InvestigationEvent::PlanRecorded(_)))
    );
    assert!(
        matches!(run.records()[3].event(), ResearchControlEvent::InvestigationAdvanced(record) if matches!(record.event(), InvestigationEvent::TaskStarted { .. }))
    );
    assert!(
        matches!(run.records()[4].event(), ResearchControlEvent::InvestigationAdvanced(record) if matches!(record.event(), InvestigationEvent::TaskCompleted { .. }))
    );
    assert!(matches!(
        run.records()[5].event(),
        ResearchControlEvent::VerificationRecorded(_)
    ));
    assert!(matches!(
        run.records()[6].event(),
        ResearchControlEvent::ResearchCompleted
    ));
    assert_eq!(
        run.records()
            .iter()
            .map(|record| record.sequence())
            .collect::<Vec<_>>(),
        [1, 2, 3, 4, 5, 6, 7]
    );
    assert_eq!(state.investigation().last_sequence(), 4);
    assert_eq!(state.investigation().research().last_sequence(), 4);
    assert_eq!(state.verification().last_sequence(), 1);
    let source = research.sources().next().unwrap();
    assert_eq!(source.content_digest(), &ContentDigest::sha256([7; 32]));
    assert!(
        research
            .evidence_items()
            .any(|evidence| evidence.excerpt() == "Exact acquired fact in context")
    );
    assert!(!model.calls[1].input.contains(&source.id().to_string()));
    assert!(!model.calls[2].input.contains(&source.id().to_string()));
    assert!(!model.calls[2].input.contains(&claim.id().to_string()));
    assert_eq!(
        ResearchControlState::reconstruct(run.records().to_vec()).unwrap(),
        *run.state()
    );
}

#[tokio::test]
async fn claims_are_verified_in_their_recorded_proposal_order() {
    let sufficient = || {
        output(json!({
            "relations":[{"evidence_index":0,"relation":"supports"}],
            "sufficiency":"sufficient"
        }))
    };
    let mut model = ScriptedModel::new([
        output(json!({"tasks":[{"objective":"query"}]})),
        output(json!({
            "evidence":[{"source_index":0,"excerpt":"Exact"}],
            "claims":[
                {"statement":"First claim","evidence_indices":[0]},
                {"statement":"Second claim","evidence_indices":[0]},
                {"statement":"Third claim","evidence_indices":[0]},
                {"statement":"Fourth claim","evidence_indices":[0]}
            ]
        })),
        sufficient(),
        sufficient(),
        sufficient(),
        sufficient(),
    ]);
    let mut retrieval = ScriptedRetrieval::new([Ok(acquired("Exact"))]);

    let run = execute(
        &mut model,
        &mut retrieval,
        request(),
        ResearchControlLimits::new(0),
        retrieved_at(),
        CancellationToken::new(),
    )
    .await
    .unwrap();

    let statements = model.calls[2..]
        .iter()
        .map(|call| {
            serde_json::from_str::<Value>(&call.input).unwrap()["claim_statement"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        statements,
        ["First claim", "Second claim", "Third claim", "Fourth claim"]
    );
    assert_eq!(run.state().status(), ResearchControlStatus::Completed);
}

#[tokio::test]
async fn malformed_planning_is_atomic_and_never_completes() {
    let run = run_with(
        [output(json!({"tasks":"not-an-array"}))],
        [],
        CancellationToken::new(),
    )
    .await;

    assert_eq!(
        run.issue(),
        Some(&ModelDrivenResearchIssue::DomainInvalidProposal)
    );
    assert_eq!(run.state().investigation().tasks().count(), 0);
    assert_noncompleted_reconstructs(&run);
}

#[tokio::test]
async fn provider_failure_is_inspectable_and_never_completes() {
    let run = run_with(
        [StructuredOutputInvocation::RequestFailure(
            ModelRequestFailure::RateLimited,
        )],
        [],
        CancellationToken::new(),
    )
    .await;

    assert_eq!(
        run.issue(),
        Some(&ModelDrivenResearchIssue::ProviderFailure(
            ModelRequestFailure::RateLimited
        ))
    );
    assert_noncompleted_reconstructs(&run);
}

#[tokio::test(start_paused = true)]
async fn model_timeout_is_distinct_and_never_completes() {
    let mut model = ScriptedModel::pending();
    let mut retrieval = ScriptedRetrieval::new([]);

    let run = execute(
        &mut model,
        &mut retrieval,
        request(),
        ResearchControlLimits::new(0),
        retrieved_at(),
        CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(run.issue(), Some(&ModelDrivenResearchIssue::ModelTimeout));
    assert_noncompleted_reconstructs(&run);
}

#[tokio::test]
async fn cancellation_is_distinct_and_operator_stopped() {
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let run = run_with([], [], cancellation).await;

    assert_eq!(run.issue(), Some(&ModelDrivenResearchIssue::Cancelled));
    assert!(matches!(
        run.state().status(),
        ResearchControlStatus::Stopped(aurora_research::ResearchStopReason::OperatorStopped)
    ));
    assert_noncompleted_reconstructs(&run);
}

#[tokio::test]
async fn cancellation_during_final_verification_cannot_become_completion() {
    let cancellation = CancellationToken::new();
    let mut model = ScriptedModel::new([
        output(json!({"tasks":[{"objective":"query"}]})),
        grounded_extraction(),
        output(json!({
            "relations":[{"evidence_index":0,"relation":"supports"}],
            "sufficiency":"sufficient"
        })),
    ])
    .cancelling_after_call(3, cancellation.clone());
    let mut retrieval = ScriptedRetrieval::new([Ok(acquired("Exact acquired fact"))]);

    let run = execute(
        &mut model,
        &mut retrieval,
        request(),
        ResearchControlLimits::new(0),
        retrieved_at(),
        cancellation,
    )
    .await
    .unwrap();

    assert_eq!(run.issue(), Some(&ModelDrivenResearchIssue::Cancelled));
    assert!(matches!(
        run.state().status(),
        ResearchControlStatus::Stopped(aurora_research::ResearchStopReason::OperatorStopped)
    ));
    assert!(
        !run.records()
            .iter()
            .any(|record| matches!(record.event(), ResearchControlEvent::ResearchCompleted))
    );
    assert_noncompleted_reconstructs(&run);
}

#[tokio::test]
async fn cancellation_during_planning_output_stops_before_tavily() {
    let cancellation = CancellationToken::new();
    let mut model = ScriptedModel::new([output(json!({"tasks":[{"objective":"query"}]}))])
        .cancelling_after_call(1, cancellation.clone());
    let mut retrieval = ScriptedRetrieval::new([]);

    let run = execute(
        &mut model,
        &mut retrieval,
        request(),
        ResearchControlLimits::new(0),
        retrieved_at(),
        cancellation,
    )
    .await
    .unwrap();

    assert_eq!(run.issue(), Some(&ModelDrivenResearchIssue::Cancelled));
    assert!(retrieval.queries.is_empty());
    assert_eq!(run.state().investigation().tasks().count(), 0);
    assert!(matches!(
        run.state().status(),
        ResearchControlStatus::Stopped(aurora_research::ResearchStopReason::OperatorStopped)
    ));
    assert_noncompleted_reconstructs(&run);
}

#[tokio::test]
async fn cancellation_after_tavily_discards_acquisition_and_fails_the_active_task() {
    let cancellation = CancellationToken::new();
    let mut model = ScriptedModel::new([output(json!({"tasks":[{"objective":"query"}]}))]);
    let mut retrieval = ScriptedRetrieval::new([Ok(acquired("must not be admitted"))])
        .cancelling_on_return(cancellation.clone());

    let run = execute(
        &mut model,
        &mut retrieval,
        request(),
        ResearchControlLimits::new(0),
        retrieved_at(),
        cancellation,
    )
    .await
    .unwrap();

    assert_eq!(run.issue(), Some(&ModelDrivenResearchIssue::Cancelled));
    assert_eq!(run.state().investigation().research().source_count(), 0);
    assert!(matches!(
        run.state().status(),
        ResearchControlStatus::Stopped(aurora_research::ResearchStopReason::OperatorStopped)
    ));
    assert_noncompleted_reconstructs(&run);
}

#[tokio::test]
async fn cancellation_observed_after_failed_tavily_takes_precedence() {
    let cancellation = CancellationToken::new();
    let mut model = ScriptedModel::new([output(json!({"tasks":[{"objective":"query"}]}))]);
    let mut retrieval = ScriptedRetrieval::new([Err(TavilyFailure::Unavailable)])
        .cancelling_on_return(cancellation.clone());

    let run = execute(
        &mut model,
        &mut retrieval,
        request(),
        ResearchControlLimits::new(0),
        retrieved_at(),
        cancellation,
    )
    .await
    .unwrap();

    assert_eq!(run.issue(), Some(&ModelDrivenResearchIssue::Cancelled));
    assert_noncompleted_reconstructs(&run);
}

#[tokio::test]
async fn valid_empty_planning_is_explicit_no_useful_action() {
    let run = run_with([output(json!({"tasks":[]}))], [], CancellationToken::new()).await;

    assert_eq!(run.issue(), Some(&ModelDrivenResearchIssue::NoUsefulAction));
    assert_eq!(run.state().investigation().tasks().count(), 0);
    assert_noncompleted_reconstructs(&run);
}

#[tokio::test]
async fn tavily_failure_records_task_and_research_failure() {
    let run = run_with(
        [output(json!({"tasks":[{"objective":"query"}]}))],
        [Err(TavilyFailure::Unavailable)],
        CancellationToken::new(),
    )
    .await;

    assert_eq!(
        run.issue(),
        Some(&ModelDrivenResearchIssue::TavilyFailure(
            TavilyFailure::Unavailable
        ))
    );
    assert!(matches!(
        run.state().status(),
        ResearchControlStatus::Failed(_)
    ));
    assert!(matches!(
        run.records().last().unwrap().event(),
        ResearchControlEvent::ResearchFailed(_)
    ));
    assert_noncompleted_reconstructs(&run);
}

#[tokio::test]
async fn every_initial_task_is_visited_in_recorded_order_after_a_failure() {
    let mut model = ScriptedModel::new([
        output(json!({
            "tasks":[{"objective":"first query"},{"objective":"second query"}]
        })),
        output(json!({"evidence":[],"claims":[]})),
    ]);
    let mut retrieval = ScriptedRetrieval::new([
        Err(TavilyFailure::Unavailable),
        Ok(acquired("second result")),
    ]);

    let run = execute(
        &mut model,
        &mut retrieval,
        request(),
        ResearchControlLimits::new(0),
        retrieved_at(),
        CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(retrieval.queries, ["first query", "second query"]);
    assert!(matches!(
        run.state().status(),
        ResearchControlStatus::Failed(_)
    ));
    let statuses = run
        .state()
        .investigation()
        .tasks()
        .map(|task| task.status())
        .collect::<Vec<_>>();
    assert!(matches!(statuses[0], InvestigationTaskStatus::Failed(_)));
    assert!(matches!(statuses[1], InvestigationTaskStatus::Completed));
    assert_noncompleted_reconstructs(&run);
}

#[tokio::test]
async fn later_cancellation_overrides_an_accumulated_task_failure() {
    let cancellation = CancellationToken::new();
    let mut model = ScriptedModel::new([output(json!({
        "tasks":[{"objective":"first query"},{"objective":"second query"}]
    }))]);
    let mut retrieval = ScriptedRetrieval::new([
        Err(TavilyFailure::Unavailable),
        Ok(acquired("must not be admitted")),
    ])
    .cancelling_after_call(2, cancellation.clone());

    let run = execute(
        &mut model,
        &mut retrieval,
        request(),
        ResearchControlLimits::new(0),
        retrieved_at(),
        cancellation,
    )
    .await
    .unwrap();

    assert_eq!(retrieval.queries, ["first query", "second query"]);
    assert_eq!(run.issue(), Some(&ModelDrivenResearchIssue::Cancelled));
    assert!(matches!(
        run.state().status(),
        ResearchControlStatus::Stopped(aurora_research::ResearchStopReason::OperatorStopped)
    ));
    assert!(
        !run.records()
            .iter()
            .any(|record| matches!(record.event(), ResearchControlEvent::ResearchCompleted))
    );
    assert_noncompleted_reconstructs(&run);
}

#[tokio::test]
async fn oversized_acquisition_fails_without_invoking_extraction_or_admitting_sources() {
    let mut model = ScriptedModel::new([output(json!({"tasks":[{"objective":"query"}]}))]);
    let mut retrieval = ScriptedRetrieval::new([Ok(acquired(&"x".repeat(1024 * 1024 + 1)))]);

    let run = execute(
        &mut model,
        &mut retrieval,
        request(),
        ResearchControlLimits::new(0),
        retrieved_at(),
        CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(
        run.issue(),
        Some(&ModelDrivenResearchIssue::SourceContentTooLarge)
    );
    assert_eq!(model.calls.len(), 1);
    assert_eq!(run.state().investigation().research().source_count(), 0);
    assert_noncompleted_reconstructs(&run);
}

#[tokio::test]
async fn valid_empty_extraction_retains_acquisition_but_blocks_without_claims() {
    let run = run_with(
        [
            output(json!({"tasks":[{"objective":"query"}]})),
            output(json!({"evidence":[],"claims":[]})),
        ],
        [Ok(acquired("complete snapshot"))],
        CancellationToken::new(),
    )
    .await;

    assert_eq!(run.issue(), Some(&ModelDrivenResearchIssue::NoUsefulAction));
    assert_eq!(run.state().investigation().research().source_count(), 1);
    assert_eq!(run.state().investigation().research().claim_count(), 0);
    assert_noncompleted_reconstructs(&run);
}

#[tokio::test]
async fn fabricated_evidence_fails_atomically_without_source_admission() {
    let run = run_with(
        [
            output(json!({"tasks":[{"objective":"query"}]})),
            output(json!({
                "evidence":[{"source_index":0,"excerpt":"fabricated"}],
                "claims":[]
            })),
        ],
        [Ok(acquired("actual snapshot"))],
        CancellationToken::new(),
    )
    .await;

    assert_eq!(run.issue(), Some(&ModelDrivenResearchIssue::EvidenceAbsent));
    assert_eq!(run.state().investigation().research().source_count(), 0);
    assert!(matches!(
        run.state().status(),
        ResearchControlStatus::Failed(_)
    ));
    assert_noncompleted_reconstructs(&run);
}

#[tokio::test]
async fn malformed_verification_does_not_record_an_assessment() {
    let run = run_with(
        [
            output(json!({"tasks":[{"objective":"query"}]})),
            grounded_extraction(),
            output(json!({"relations":[],"sufficiency":"sufficient"})),
        ],
        [Ok(acquired("Exact acquired fact"))],
        CancellationToken::new(),
    )
    .await;

    assert_eq!(
        run.issue(),
        Some(&ModelDrivenResearchIssue::DomainInvalidProposal)
    );
    assert_eq!(run.state().verification().assessments().count(), 0);
    assert!(matches!(
        run.state().status(),
        ResearchControlStatus::Failed(_)
    ));
    assert_noncompleted_reconstructs(&run);
}

#[tokio::test]
async fn invalid_extraction_indexes_fail_without_partial_admission() {
    let run = run_with(
        [
            output(json!({"tasks":[{"objective":"query"}]})),
            output(json!({
                "evidence":[{"source_index":3,"excerpt":"Exact"}],
                "claims":[]
            })),
        ],
        [Ok(acquired("Exact"))],
        CancellationToken::new(),
    )
    .await;

    assert_eq!(
        run.issue(),
        Some(&ModelDrivenResearchIssue::DomainInvalidProposal)
    );
    assert_eq!(run.state().investigation().research().source_count(), 0);
    assert_noncompleted_reconstructs(&run);
}

#[tokio::test]
async fn invalid_verification_indexes_fail_without_partial_assessment() {
    let run = run_with(
        [
            output(json!({"tasks":[{"objective":"query"}]})),
            grounded_extraction(),
            output(json!({
                "relations":[{"evidence_index":99,"relation":"supports"}],
                "sufficiency":"sufficient"
            })),
        ],
        [Ok(acquired("Exact acquired fact"))],
        CancellationToken::new(),
    )
    .await;

    assert_eq!(
        run.issue(),
        Some(&ModelDrivenResearchIssue::DomainInvalidProposal)
    );
    assert_eq!(run.state().verification().assessments().count(), 0);
    assert_noncompleted_reconstructs(&run);
}

#[tokio::test]
async fn insufficient_verification_with_no_allowance_stops_exhausted() {
    let run = run_with(
        [
            output(json!({"tasks":[{"objective":"query"}]})),
            grounded_extraction(),
            output(json!({
                "relations":[{"evidence_index":0,"relation":"supports"}],
                "sufficiency":"insufficient"
            })),
        ],
        [Ok(acquired("Exact acquired fact"))],
        CancellationToken::new(),
    )
    .await;

    assert_eq!(
        run.issue(),
        Some(&ModelDrivenResearchIssue::FollowUpAllowanceExhausted)
    );
    assert!(matches!(
        run.state().status(),
        ResearchControlStatus::Stopped(aurora_research::ResearchStopReason::BudgetExhausted)
    ));
    assert_eq!(run.state().gaps().count(), 1);
    assert_noncompleted_reconstructs(&run);
}

#[tokio::test]
async fn adaptive_follow_up_resolves_exact_gap_and_completes_from_reconstructed_state() {
    let mut model = ScriptedModel::new([
        output(json!({"tasks":[{"objective":"initial query"}]})),
        grounded_extraction(),
        output(json!({
            "relations":[{"evidence_index":0,"relation":"supports"}],
            "sufficiency":"insufficient"
        })),
        output(json!({"objective":"exact follow-up query"})),
        output(json!({
            "evidence":[{"source_index":0,"excerpt":"Decisive follow-up fact"}],
            "claims":[]
        })),
        output(json!({
            "relations":[{"evidence_index":2,"relation":"supports"}],
            "sufficiency":"sufficient"
        })),
    ]);
    let mut retrieval = ScriptedRetrieval::new([
        Ok(acquired("Exact acquired fact")),
        Ok(acquired_at("Decisive follow-up fact", 5)),
    ]);

    let run = execute(
        &mut model,
        &mut retrieval,
        request(),
        ResearchControlLimits::new(1),
        retrieved_at(),
        CancellationToken::new(),
    )
    .await
    .expect("generated records remain valid");

    assert_eq!(run.issue(), None);
    assert_eq!(run.state().status(), ResearchControlStatus::Completed);
    assert_eq!(
        retrieval.queries,
        ["initial query", "exact follow-up query"]
    );
    assert_eq!(model.calls.len(), 6);
    assert_eq!(model.calls[3].name, "follow_up_planning");
    let follow_up_input: Value = serde_json::from_str(&model.calls[3].input).unwrap();
    assert_eq!(
        follow_up_input["gap"],
        "verification requires additional evidence"
    );
    assert_eq!(follow_up_input["claim_statement"], "The fact is grounded");
    assert_eq!(follow_up_input["remaining_follow_ups"], 1);
    assert_eq!(
        follow_up_input["completed_objectives"],
        json!(["initial query"])
    );

    let follow_up = run
        .state()
        .investigation()
        .tasks()
        .nth(1)
        .expect("one follow-up task");
    assert_eq!(follow_up.task().objective(), "exact follow-up query");
    assert!(matches!(
        follow_up.status(),
        InvestigationTaskStatus::Completed
    ));
    let TaskOrigin::FollowUp { gap, .. } = follow_up.task().origin() else {
        panic!("adaptive task retains follow-up origin");
    };
    assert_eq!(gap.as_str(), "verification requires additional evidence");

    let verification_input: Value = serde_json::from_str(&model.calls[5].input).unwrap();
    assert_eq!(
        verification_input["claim_statement"],
        "The fact is grounded"
    );
    assert!(
        verification_input["evidence"]
            .as_array()
            .is_some_and(|items| items
                .iter()
                .any(|item| { item["excerpt"] == "Decisive follow-up fact" }))
    );
    assert_eq!(run.state().verification().assessments().count(), 2);
    assert!(
        run.state()
            .gaps()
            .all(|gap| matches!(gap.status(), ResearchGapStatus::Resolved(_)))
    );
    assert!(run.records().iter().any(|record| matches!(
        record.event(),
        ResearchControlEvent::GapFollowUpRecorded { .. }
    )));
    assert!(
        run.records()
            .iter()
            .any(|record| matches!(record.event(), ResearchControlEvent::GapResolved { .. }))
    );
    assert!(matches!(
        run.records().last().unwrap().event(),
        ResearchControlEvent::ResearchCompleted
    ));
    assert_eq!(
        ResearchControlState::reconstruct(run.records().to_vec()).unwrap(),
        *run.state()
    );
}

#[tokio::test]
async fn adaptive_contradictory_mixed_evidence_requires_a_gap_and_never_completes() {
    let run = run_with(
        [
            output(json!({"tasks":[{"objective":"query"}]})),
            grounded_extraction(),
            output(json!({
                "relations":[
                    {"evidence_index":0,"relation":"supports"},
                    {"evidence_index":1,"relation":"contradicts"}
                ],
                "sufficiency":"sufficient"
            })),
        ],
        [Ok(acquired("Exact acquired fact"))],
        CancellationToken::new(),
    )
    .await;

    assert_eq!(
        run.issue(),
        Some(&ModelDrivenResearchIssue::FollowUpAllowanceExhausted)
    );
    assert_eq!(run.state().gaps().count(), 1);
    assert_noncompleted_reconstructs(&run);
}

#[tokio::test]
async fn adaptive_successive_insufficient_assessments_link_newest_gap_and_resolve_all_open_gaps() {
    let mut model = ScriptedModel::new([
        output(json!({"tasks":[{"objective":"initial query"}]})),
        grounded_extraction(),
        insufficient_verification(),
        output(json!({"objective":"first follow-up query"})),
        follow_up_extraction("First follow-up fact"),
        insufficient_verification(),
        output(json!({"objective":"second follow-up query"})),
        follow_up_extraction("Second follow-up fact"),
        sufficient_verification(),
    ]);
    let mut retrieval = ScriptedRetrieval::new([
        Ok(acquired("Exact acquired fact")),
        Ok(acquired_at("First follow-up fact", 5)),
        Ok(acquired_at("Second follow-up fact", 8)),
    ]);

    let run = execute(
        &mut model,
        &mut retrieval,
        request(),
        ResearchControlLimits::new(2),
        retrieved_at(),
        CancellationToken::new(),
    )
    .await
    .expect("generated records remain valid");

    assert_eq!(run.issue(), None);
    assert_eq!(run.state().status(), ResearchControlStatus::Completed);
    assert_eq!(run.state().follow_up_count(), 2);
    let verification_ids = run
        .records()
        .iter()
        .filter_map(|record| match record.event() {
            ResearchControlEvent::VerificationRecorded(record) => Some(*record.assessment().id()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let gap_causes = run
        .records()
        .iter()
        .filter_map(|record| match record.event() {
            ResearchControlEvent::GapIdentified(gap) => Some(gap.cause().clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(gap_causes.len(), 2);
    assert_eq!(
        gap_causes[0],
        ResearchGapCause::Verification(verification_ids[0])
    );
    assert_eq!(
        gap_causes[1],
        ResearchGapCause::Verification(verification_ids[1])
    );
    let resolved = run
        .records()
        .iter()
        .filter_map(|record| match record.event() {
            ResearchControlEvent::GapResolved {
                verification_id, ..
            } => Some(*verification_id),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(resolved, [verification_ids[2], verification_ids[2]]);
    assert!(run.state().gaps().all(
        |gap| matches!(gap.status(), ResearchGapStatus::Resolved(id) if id == &verification_ids[2])
    ));
    let second_follow_up: Value = serde_json::from_str(&model.calls[6].input).unwrap();
    assert_eq!(second_follow_up["remaining_follow_ups"], 1);
    assert_eq!(
        second_follow_up["completed_objectives"],
        json!(["initial query", "first follow-up query"])
    );
    assert_eq!(
        ResearchControlState::reconstruct(run.records().to_vec()).unwrap(),
        *run.state()
    );
}

#[tokio::test]
async fn adaptive_follow_up_reverifies_affected_claim_and_only_assesses_new_claims_once() {
    let mut model = ScriptedModel::new([
        output(json!({"tasks":[{"objective":"initial query"}]})),
        grounded_extraction(),
        insufficient_verification(),
        output(json!({"objective":"follow-up query"})),
        output(json!({
            "evidence":[{"source_index":0,"excerpt":"Follow-up fact"}],
            "claims":[{"statement":"A new follow-up claim","evidence_indices":[0]}]
        })),
        sufficient_verification(),
        sufficient_verification(),
    ]);
    let mut retrieval = ScriptedRetrieval::new([
        Ok(acquired("Exact acquired fact")),
        Ok(acquired_at("Follow-up fact", 5)),
    ]);

    let run = execute(
        &mut model,
        &mut retrieval,
        request(),
        ResearchControlLimits::new(1),
        retrieved_at(),
        CancellationToken::new(),
    )
    .await
    .expect("valid follow-up claims remain executable");

    assert_eq!(run.issue(), None);
    assert_eq!(run.state().status(), ResearchControlStatus::Completed);
    let verified_statements = model
        .calls
        .iter()
        .filter(|call| call.name == "claim_verification")
        .map(|call| {
            serde_json::from_str::<Value>(&call.input).unwrap()["claim_statement"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        verified_statements,
        [
            "The fact is grounded",
            "The fact is grounded",
            "A new follow-up claim"
        ]
    );
    assert_eq!(run.state().verification().assessments().count(), 3);
    assert_eq!(
        ResearchControlState::reconstruct(run.records().to_vec()).unwrap(),
        *run.state()
    );
}

#[tokio::test]
async fn adaptive_new_claim_insufficiency_drives_its_exact_gap_then_completes() {
    let mut model = ScriptedModel::new([
        output(json!({"tasks":[{"objective":"initial query"}]})),
        grounded_extraction(),
        insufficient_verification(),
        output(json!({"objective":"original-claim follow-up"})),
        output(json!({
            "evidence":[{"source_index":0,"excerpt":"New claim evidence"}],
            "claims":[{"statement":"New adaptive claim","evidence_indices":[0]}]
        })),
        sufficient_verification(),
        insufficient_verification(),
        output(json!({"objective":"new-claim follow-up"})),
        follow_up_extraction("Decisive new claim evidence"),
        sufficient_verification(),
    ]);
    let mut retrieval = ScriptedRetrieval::new([
        Ok(acquired("Exact acquired fact")),
        Ok(acquired_at("New claim evidence", 5)),
        Ok(acquired_at("Decisive new claim evidence", 9)),
    ]);

    let run = execute(
        &mut model,
        &mut retrieval,
        request(),
        ResearchControlLimits::new(2),
        retrieved_at(),
        CancellationToken::new(),
    )
    .await
    .expect("newly admitted claim retains an exact adaptive gap cause");

    assert_eq!(run.issue(), None);
    assert_eq!(run.state().status(), ResearchControlStatus::Completed);
    let verification_ids = run
        .records()
        .iter()
        .filter_map(|record| match record.event() {
            ResearchControlEvent::VerificationRecorded(record) => Some(*record.assessment().id()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let gap_causes = run
        .records()
        .iter()
        .filter_map(|record| match record.event() {
            ResearchControlEvent::GapIdentified(gap) => match gap.cause() {
                ResearchGapCause::Verification(id) => Some(*id),
                ResearchGapCause::InvestigationFailure(_) => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(gap_causes, [verification_ids[0], verification_ids[2]]);
    assert_eq!(
        gap_causes
            .iter()
            .filter(|id| id == &&verification_ids[2])
            .count(),
        1
    );
    let new_claim_follow_up: Value = serde_json::from_str(&model.calls[7].input).unwrap();
    assert_eq!(new_claim_follow_up["claim_statement"], "New adaptive claim");
    assert_eq!(retrieval.queries.last().unwrap(), "new-claim follow-up");
    assert_eq!(run.state().follow_up_count(), 2);
    assert!(
        run.state()
            .gaps()
            .all(|gap| matches!(gap.status(), ResearchGapStatus::Resolved(_)))
    );
    assert_eq!(
        ResearchControlState::reconstruct(run.records().to_vec()).unwrap(),
        *run.state()
    );
}

#[tokio::test]
async fn adaptive_multiple_claims_consume_each_exact_pending_gap_cause_once() {
    let mut model = ScriptedModel::new([
        output(json!({"tasks":[{"objective":"initial query"}]})),
        output(json!({
            "evidence":[{"source_index":0,"excerpt":"Exact acquired fact"}],
            "claims":[
                {"statement":"First claim","evidence_indices":[0]},
                {"statement":"Second claim","evidence_indices":[0]}
            ]
        })),
        insufficient_verification(),
        insufficient_verification(),
        output(json!({"objective":"first follow-up query"})),
        follow_up_extraction("First follow-up fact"),
        sufficient_verification(),
        output(json!({"objective":"second follow-up query"})),
        follow_up_extraction("Second follow-up fact"),
        sufficient_verification(),
    ]);
    let mut retrieval = ScriptedRetrieval::new([
        Ok(acquired("Exact acquired fact")),
        Ok(acquired_at("First follow-up fact", 6)),
        Ok(acquired_at("Second follow-up fact", 9)),
    ]);

    let run = execute(
        &mut model,
        &mut retrieval,
        request(),
        ResearchControlLimits::new(2),
        retrieved_at(),
        CancellationToken::new(),
    )
    .await
    .expect("each recorded claim keeps its exact pending assessment");

    assert_eq!(run.issue(), None);
    assert_eq!(run.state().status(), ResearchControlStatus::Completed);
    let followed_claims = [4, 7]
        .map(|call| {
            serde_json::from_str::<Value>(&model.calls[call].input).unwrap()["claim_statement"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        followed_claims,
        ["First claim".to_owned(), "Second claim".to_owned()].into()
    );
    let initial_verifications = run
        .records()
        .iter()
        .filter_map(|record| match record.event() {
            ResearchControlEvent::VerificationRecorded(record) => Some(*record.assessment().id()),
            _ => None,
        })
        .take(2)
        .collect::<std::collections::BTreeSet<_>>();
    let gap_causes = run
        .records()
        .iter()
        .filter_map(|record| match record.event() {
            ResearchControlEvent::GapIdentified(gap) => match gap.cause() {
                ResearchGapCause::Verification(id) => Some(*id),
                ResearchGapCause::InvestigationFailure(_) => None,
            },
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(gap_causes, initial_verifications);
    assert_eq!(run.state().gaps().count(), 2);
    assert!(
        run.state()
            .gaps()
            .all(|gap| matches!(gap.status(), ResearchGapStatus::Resolved(_)))
    );
}

#[tokio::test]
async fn adaptive_follow_up_limit_exhaustion_records_new_gap_without_inferred_completion() {
    let mut model = ScriptedModel::new([
        output(json!({"tasks":[{"objective":"initial query"}]})),
        grounded_extraction(),
        insufficient_verification(),
        output(json!({"objective":"follow-up query"})),
        follow_up_extraction("Follow-up fact"),
        insufficient_verification(),
    ]);
    let mut retrieval = ScriptedRetrieval::new([
        Ok(acquired("Exact acquired fact")),
        Ok(acquired_at("Follow-up fact", 5)),
    ]);

    let run = execute(
        &mut model,
        &mut retrieval,
        request(),
        ResearchControlLimits::new(1),
        retrieved_at(),
        CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(
        run.issue(),
        Some(&ModelDrivenResearchIssue::FollowUpAllowanceExhausted)
    );
    assert_eq!(run.state().follow_up_count(), 1);
    assert_eq!(run.state().gaps().count(), 2);
    assert!(matches!(
        run.state().status(),
        ResearchControlStatus::Stopped(aurora_research::ResearchStopReason::BudgetExhausted)
    ));
    assert_noncompleted_reconstructs(&run);
}

#[tokio::test]
async fn adaptive_null_follow_up_stops_blocked_without_recording_a_task() {
    let mut model = ScriptedModel::new([
        output(json!({"tasks":[{"objective":"query"}]})),
        grounded_extraction(),
        insufficient_verification(),
        output(json!({"objective":null})),
    ]);
    let mut retrieval = ScriptedRetrieval::new([Ok(acquired("Exact acquired fact"))]);

    let run = execute(
        &mut model,
        &mut retrieval,
        request(),
        ResearchControlLimits::new(1),
        retrieved_at(),
        CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(run.issue(), Some(&ModelDrivenResearchIssue::NoUsefulAction));
    assert_eq!(run.state().follow_up_count(), 0);
    assert_eq!(run.state().investigation().tasks().count(), 1);
    assert!(matches!(
        run.state().status(),
        ResearchControlStatus::Stopped(aurora_research::ResearchStopReason::Blocked(_))
    ));
    assert_noncompleted_reconstructs(&run);
}

#[tokio::test]
async fn adaptive_malformed_follow_up_fails_without_recording_a_task() {
    let mut model = ScriptedModel::new([
        output(json!({"tasks":[{"objective":"query"}]})),
        grounded_extraction(),
        insufficient_verification(),
        output(json!({"objective":["not a query"]})),
    ]);
    let mut retrieval = ScriptedRetrieval::new([Ok(acquired("Exact acquired fact"))]);

    let run = execute(
        &mut model,
        &mut retrieval,
        request(),
        ResearchControlLimits::new(1),
        retrieved_at(),
        CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(
        run.issue(),
        Some(&ModelDrivenResearchIssue::DomainInvalidProposal)
    );
    assert_eq!(run.state().follow_up_count(), 0);
    assert!(matches!(
        run.state().status(),
        ResearchControlStatus::Failed(_)
    ));
    assert_noncompleted_reconstructs(&run);
}

#[tokio::test]
async fn adaptive_follow_up_retrieval_failure_records_failed_task_and_research_failure() {
    let mut model = ScriptedModel::new([
        output(json!({"tasks":[{"objective":"query"}]})),
        grounded_extraction(),
        insufficient_verification(),
        output(json!({"objective":"follow-up query"})),
    ]);
    let mut retrieval = ScriptedRetrieval::new([
        Ok(acquired("Exact acquired fact")),
        Err(TavilyFailure::Unavailable),
    ]);

    let run = execute(
        &mut model,
        &mut retrieval,
        request(),
        ResearchControlLimits::new(1),
        retrieved_at(),
        CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(
        run.issue(),
        Some(&ModelDrivenResearchIssue::TavilyFailure(
            TavilyFailure::Unavailable
        ))
    );
    assert!(matches!(
        run.state().status(),
        ResearchControlStatus::Failed(_)
    ));
    assert!(matches!(
        run.state().investigation().tasks().nth(1).unwrap().status(),
        InvestigationTaskStatus::Failed(_)
    ));
    assert_noncompleted_reconstructs(&run);
}

#[tokio::test]
async fn adaptive_follow_up_model_failure_is_explicit_and_never_completes() {
    let mut model = ScriptedModel::new([
        output(json!({"tasks":[{"objective":"query"}]})),
        grounded_extraction(),
        insufficient_verification(),
        StructuredOutputInvocation::RequestFailure(ModelRequestFailure::ServiceUnavailable),
    ]);
    let mut retrieval = ScriptedRetrieval::new([Ok(acquired("Exact acquired fact"))]);

    let run = execute(
        &mut model,
        &mut retrieval,
        request(),
        ResearchControlLimits::new(1),
        retrieved_at(),
        CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(
        run.issue(),
        Some(&ModelDrivenResearchIssue::ProviderFailure(
            ModelRequestFailure::ServiceUnavailable
        ))
    );
    assert!(matches!(
        run.state().status(),
        ResearchControlStatus::Failed(_)
    ));
    assert_noncompleted_reconstructs(&run);
}

#[tokio::test]
async fn adaptive_unresolved_verification_creates_a_new_gap_before_null_follow_up_blocks() {
    let mut model = ScriptedModel::new([
        output(json!({"tasks":[{"objective":"query"}]})),
        grounded_extraction(),
        insufficient_verification(),
        output(json!({"objective":"first follow-up query"})),
        follow_up_extraction("Follow-up fact"),
        output(json!({
            "relations":[{"evidence_index":0,"relation":"unclear"}],
            "sufficiency":"indeterminate"
        })),
        output(json!({"objective":null})),
    ]);
    let mut retrieval = ScriptedRetrieval::new([
        Ok(acquired("Exact acquired fact")),
        Ok(acquired_at("Follow-up fact", 5)),
    ]);

    let run = execute(
        &mut model,
        &mut retrieval,
        request(),
        ResearchControlLimits::new(2),
        retrieved_at(),
        CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(run.issue(), Some(&ModelDrivenResearchIssue::NoUsefulAction));
    assert_eq!(run.state().follow_up_count(), 1);
    assert_eq!(run.state().gaps().count(), 2);
    assert!(
        run.state()
            .gaps()
            .all(|gap| matches!(gap.status(), ResearchGapStatus::Open))
    );
    assert_noncompleted_reconstructs(&run);
}

#[tokio::test]
async fn adaptive_cancellation_during_follow_up_is_operator_stopped_after_task_failure() {
    let cancellation = CancellationToken::new();
    let mut model = ScriptedModel::new([
        output(json!({"tasks":[{"objective":"query"}]})),
        grounded_extraction(),
        insufficient_verification(),
        output(json!({"objective":"follow-up query"})),
    ]);
    let mut retrieval = ScriptedRetrieval::new([
        Ok(acquired("Exact acquired fact")),
        Ok(acquired_at("must not be admitted", 5)),
    ])
    .cancelling_after_call(2, cancellation.clone());

    let run = execute(
        &mut model,
        &mut retrieval,
        request(),
        ResearchControlLimits::new(1),
        retrieved_at(),
        cancellation,
    )
    .await
    .unwrap();

    assert_eq!(run.issue(), Some(&ModelDrivenResearchIssue::Cancelled));
    assert!(matches!(
        run.state().status(),
        ResearchControlStatus::Stopped(aurora_research::ResearchStopReason::OperatorStopped)
    ));
    assert!(matches!(
        run.state().investigation().tasks().nth(1).unwrap().status(),
        InvestigationTaskStatus::Failed(_)
    ));
    assert_eq!(run.state().investigation().research().source_count(), 1);
    assert_noncompleted_reconstructs(&run);
}

#[test]
fn adaptive_cancelled_decision_boundary_records_operator_stop_and_never_completion() {
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let mut owner = SequentialRun::default();
    owner
        .append(ResearchControlEvent::LimitsRecorded(
            ResearchControlLimits::new(1),
        ))
        .unwrap();
    owner
        .append_investigation(InvestigationEvent::RequestRecorded(request()))
        .unwrap();

    assert!(owner.stop_if_cancelled(&cancellation).unwrap());
    let run = owner.finish();

    assert_eq!(run.issue(), Some(&ModelDrivenResearchIssue::Cancelled));
    assert!(matches!(
        run.state().status(),
        ResearchControlStatus::Stopped(aurora_research::ResearchStopReason::OperatorStopped)
    ));
    assert!(
        !run.records()
            .iter()
            .any(|record| matches!(record.event(), ResearchControlEvent::ResearchCompleted))
    );
    assert_noncompleted_reconstructs(&run);
}

fn insufficient_verification() -> StructuredOutputInvocation {
    output(json!({
        "relations":[{"evidence_index":0,"relation":"supports"}],
        "sufficiency":"insufficient"
    }))
}

fn sufficient_verification() -> StructuredOutputInvocation {
    output(json!({
        "relations":[{"evidence_index":0,"relation":"supports"}],
        "sufficiency":"sufficient"
    }))
}

fn follow_up_extraction(excerpt: &str) -> StructuredOutputInvocation {
    output(json!({
        "evidence":[{"source_index":0,"excerpt":excerpt}],
        "claims":[]
    }))
}

fn grounded_extraction() -> StructuredOutputInvocation {
    output(json!({
        "evidence":[{"source_index":0,"excerpt":"Exact acquired fact"}],
        "claims":[{"statement":"The fact is grounded","evidence_indices":[0]}]
    }))
}

async fn run_with<const M: usize, const R: usize>(
    model: [StructuredOutputInvocation; M],
    retrieval: [Result<InvestigationResult, TavilyFailure>; R],
    cancellation: CancellationToken,
) -> super::ModelDrivenResearchRun {
    let mut model = ScriptedModel::new(model);
    let mut retrieval = ScriptedRetrieval::new(retrieval);
    execute(
        &mut model,
        &mut retrieval,
        request(),
        ResearchControlLimits::new(0),
        retrieved_at(),
        cancellation,
    )
    .await
    .expect("generated records remain valid")
}

fn assert_noncompleted_reconstructs(run: &super::ModelDrivenResearchRun) {
    assert_ne!(run.state().status(), ResearchControlStatus::Completed);
    assert!(run.issue().is_some());
    assert_eq!(
        ResearchControlState::reconstruct(run.records().to_vec()).unwrap(),
        *run.state()
    );
}

fn output(value: Value) -> StructuredOutputInvocation {
    StructuredOutputInvocation::Output(value)
}

fn request() -> ResearchRequest {
    ResearchRequest::new("What is the fact?".to_owned()).unwrap()
}

fn retrieved_at() -> RetrievedAt {
    RetrievedAt::new("2026-08-29T12:34:56Z").unwrap()
}

fn acquired(text: &str) -> InvestigationResult {
    acquired_at(text, 1)
}

fn acquired_at(text: &str, first_sequence: u64) -> InvestigationResult {
    let source_id = SourceId::generate();
    let source = Source::new(
        source_id,
        ContentDigest::sha256([7; 32]),
        "https://source.example/article".to_owned(),
        Some("Source title".to_owned()),
        retrieved_at(),
        MediaType::new("text/plain").unwrap(),
    )
    .unwrap();
    let full = Evidence::new(EvidenceId::generate(), source_id, text.to_owned()).unwrap();
    InvestigationResult::new(vec![
        ResearchRecord::new(first_sequence, ResearchEvent::SourceRecorded(source)).unwrap(),
        ResearchRecord::new(
            first_sequence.checked_add(1).unwrap(),
            ResearchEvent::EvidenceRecorded(full),
        )
        .unwrap(),
    ])
}

struct ScriptedModel {
    responses: VecDeque<StructuredOutputInvocation>,
    calls: Vec<ModelProposalRequest>,
    pending: bool,
    cancel_after_call: Option<(usize, CancellationToken)>,
}

impl ScriptedModel {
    fn new<const N: usize>(responses: [StructuredOutputInvocation; N]) -> Self {
        Self {
            responses: responses.into(),
            calls: Vec::new(),
            pending: false,
            cancel_after_call: None,
        }
    }

    fn pending() -> Self {
        Self {
            responses: VecDeque::new(),
            calls: Vec::new(),
            pending: true,
            cancel_after_call: None,
        }
    }

    fn cancelling_after_call(mut self, call: usize, cancellation: CancellationToken) -> Self {
        self.cancel_after_call = Some((call, cancellation));
        self
    }
}

impl ProposalModel for ScriptedModel {
    fn propose(
        &mut self,
        request: ModelProposalRequest,
        _cancellation: CancellationToken,
    ) -> ProposalFuture {
        self.calls.push(request);
        if self.pending {
            Box::pin(pending())
        } else {
            let response = self
                .responses
                .pop_front()
                .expect("the scripted model has a response");
            let cancellation = self
                .cancel_after_call
                .as_ref()
                .filter(|(call, _)| self.calls.len() == *call)
                .map(|(_, cancellation)| cancellation.clone());
            Box::pin(async move {
                if let Some(cancellation) = cancellation {
                    cancellation.cancel();
                }
                response
            })
        }
    }
}

struct ScriptedRetrieval {
    responses: VecDeque<Result<InvestigationResult, TavilyFailure>>,
    queries: Vec<String>,
    cancel_after_call: Option<(usize, CancellationToken)>,
}

impl ScriptedRetrieval {
    fn new<const N: usize>(responses: [Result<InvestigationResult, TavilyFailure>; N]) -> Self {
        Self {
            responses: responses.into(),
            queries: Vec::new(),
            cancel_after_call: None,
        }
    }

    fn cancelling_on_return(mut self, cancellation: CancellationToken) -> Self {
        self.cancel_after_call = Some((1, cancellation));
        self
    }

    fn cancelling_after_call(mut self, call: usize, cancellation: CancellationToken) -> Self {
        self.cancel_after_call = Some((call, cancellation));
        self
    }
}

impl ResearchRetrieval for ScriptedRetrieval {
    fn retrieve<'a>(
        &'a mut self,
        task: &'a InvestigationTask,
        _next_sequence: u64,
        _retrieved_at: RetrievedAt,
    ) -> RetrievalFuture<'a> {
        self.queries.push(task.objective().to_owned());
        if let Some((call, cancellation)) = &self.cancel_after_call
            && self.queries.len() == *call
        {
            cancellation.cancel();
        }
        Box::pin(ready(
            self.responses
                .pop_front()
                .expect("the scripted retrieval has a response"),
        ))
    }
}

fn _future_types_are_send(
    proposal: Pin<Box<dyn Future<Output = StructuredOutputInvocation> + Send>>,
    retrieval: Pin<Box<dyn Future<Output = Result<InvestigationResult, TavilyFailure>> + Send>>,
) {
    drop((proposal, retrieval));
}
