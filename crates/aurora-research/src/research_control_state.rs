use std::collections::BTreeMap;

use crate::{
    ClaimId, EvidenceRelation, EvidenceSufficiency, IdentifiedResearchGap, InvestigationEvent,
    InvestigationRecord, InvestigationState, InvestigationStatus, InvestigationTaskId,
    InvestigationTaskStatus, InvestigationTransitionError, ResearchControlEvent,
    ResearchControlLimits, ResearchControlRecord, ResearchFailure, ResearchGapCause, ResearchGapId,
    ResearchStopReason, TaskOrigin, VerificationAssessment, VerificationId, VerificationState,
    VerificationTransitionError,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResearchGapStatus {
    Open,
    Resolved(VerificationId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResearchGapState {
    gap: IdentifiedResearchGap,
    identified_sequence: u64,
    follow_up_task_id: Option<InvestigationTaskId>,
    status: ResearchGapStatus,
}

impl ResearchGapState {
    pub const fn gap(&self) -> &IdentifiedResearchGap {
        &self.gap
    }

    pub const fn follow_up_task_id(&self) -> Option<&InvestigationTaskId> {
        self.follow_up_task_id.as_ref()
    }

    pub const fn status(&self) -> &ResearchGapStatus {
        &self.status
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResearchControlStatus {
    AwaitingLimits,
    Researching,
    AwaitingNextStep,
    Completed,
    Failed(ResearchFailure),
    Stopped(ResearchStopReason),
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ResearchControlTransitionError {
    #[error("research control sequence is exhausted")]
    SequenceExhausted,
    #[error("expected research control record sequence {expected}, found {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("research control limits are required")]
    LimitsRequired,
    #[error("research control limits are already present")]
    DuplicateLimits,
    #[error("research control is already terminal")]
    ResearchAlreadyTerminal,
    #[error("nested investigation transition failed: {0}")]
    Investigation(InvestigationTransitionError),
    #[error("nested verification transition failed: {0}")]
    Verification(VerificationTransitionError),
    #[error("follow-up investigation must be linked to an identified gap")]
    UnlinkedFollowUp,
    #[error("research gap identifier {0} is already present")]
    DuplicateGap(ResearchGapId),
    #[error("research gap cause is already present")]
    DuplicateGapCause,
    #[error("verification identifier {0} is not present")]
    UnknownVerificationCause(VerificationId),
    #[error("verification assessment {0} does not require follow-up")]
    VerificationDoesNotRequireFollowUp(VerificationId),
    #[error("investigation task identifier {0} is not present")]
    UnknownInvestigationCause(InvestigationTaskId),
    #[error("investigation task {0} is not failed")]
    InvestigationTaskNotFailed(InvestigationTaskId),
    #[error("research gap identifier {0} is not present")]
    UnknownGap(ResearchGapId),
    #[error("research gap identifier {0} is already resolved")]
    GapAlreadyResolved(ResearchGapId),
    #[error("research gap identifier {0} already has follow-up work")]
    GapAlreadyHasFollowUp(ResearchGapId),
    #[error("gap follow-up record does not contain a follow-up task")]
    GapFollowUpRecordRequired,
    #[error("follow-up text does not match research gap {0}")]
    FollowUpGapMismatch(ResearchGapId),
    #[error("research follow-up limit {limit} is exhausted")]
    FollowUpLimitReached { limit: u32 },
    #[error("failed investigation task {0} cannot parent adaptive follow-up work")]
    InvestigationFailureCannotReplan(InvestigationTaskId),
    #[error("research gap {0} has no follow-up task")]
    GapHasNoFollowUp(ResearchGapId),
    #[error("follow-up investigation task {0} is not completed")]
    FollowUpNotCompleted(InvestigationTaskId),
    #[error("resolution verification identifier {0} is not present")]
    UnknownResolutionVerification(VerificationId),
    #[error("resolution verification {0} was not recorded after follow-up completion")]
    ResolutionPrecedesFollowUp(VerificationId),
    #[error("resolution verification targets claim {actual}, expected {expected}")]
    ResolutionClaimMismatch { expected: ClaimId, actual: ClaimId },
    #[error("verification assessment {0} does not resolve the research gap")]
    VerificationDoesNotResolveGap(VerificationId),
    #[error("research work is not ready for a terminal control outcome")]
    ResearchWorkRemaining,
    #[error("research has no proposed claims")]
    NoClaims,
    #[error("claim {0} has no control-ready assessment")]
    ClaimNeedsAssessment(ClaimId),
    #[error("verification assessment {0} has no identified gap")]
    VerificationNeedsGap(VerificationId),
    #[error("failed investigation task {0} prevents successful completion")]
    InvestigationFailurePreventsCompletion(InvestigationTaskId),
    #[error("research gap {0} remains open")]
    OpenGap(ResearchGapId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ResearchControlTerminal {
    Completed,
    Failed(ResearchFailure),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResearchControlState {
    last_sequence: u64,
    limits: Option<ResearchControlLimits>,
    investigation: InvestigationState,
    verification: VerificationState,
    gaps: BTreeMap<ResearchGapId, ResearchGapState>,
    follow_up_count: u32,
    verification_sequences: BTreeMap<VerificationId, u64>,
    task_completion_sequences: BTreeMap<InvestigationTaskId, u64>,
    terminal: Option<ResearchControlTerminal>,
}

impl ResearchControlState {
    pub fn reconstruct<I>(records: I) -> Result<Self, ResearchControlTransitionError>
    where
        I: IntoIterator<Item = ResearchControlRecord>,
    {
        let mut state = Self::default();
        for record in records {
            state.apply(record)?;
        }
        Ok(state)
    }

    pub fn apply(
        &mut self,
        record: ResearchControlRecord,
    ) -> Result<(), ResearchControlTransitionError> {
        let expected = self
            .last_sequence
            .checked_add(1)
            .ok_or(ResearchControlTransitionError::SequenceExhausted)?;
        if record.sequence() != expected {
            return Err(ResearchControlTransitionError::Sequence {
                expected,
                actual: record.sequence(),
            });
        }
        if self.is_terminal() {
            return Err(ResearchControlTransitionError::ResearchAlreadyTerminal);
        }

        let sequence = record.sequence();
        let event = record.into_event();
        if self.limits.is_none() && !matches!(event, ResearchControlEvent::LimitsRecorded(_)) {
            return Err(ResearchControlTransitionError::LimitsRequired);
        }
        if self.limits.is_some() && matches!(event, ResearchControlEvent::LimitsRecorded(_)) {
            return Err(ResearchControlTransitionError::DuplicateLimits);
        }

        match event {
            ResearchControlEvent::LimitsRecorded(limits) => self.limits = Some(limits),
            ResearchControlEvent::InvestigationAdvanced(record) => {
                self.apply_investigation(sequence, record)?;
            }
            ResearchControlEvent::VerificationRecorded(record) => {
                self.apply_verification(sequence, record)?;
            }
            ResearchControlEvent::GapIdentified(gap) => self.identify_gap(sequence, gap)?,
            ResearchControlEvent::GapFollowUpRecorded {
                gap_id,
                investigation_record,
            } => self.record_gap_follow_up(gap_id, investigation_record)?,
            ResearchControlEvent::GapResolved {
                gap_id,
                verification_id,
            } => {
                self.resolve_gap(gap_id, verification_id)?;
            }
            ResearchControlEvent::ResearchCompleted => {
                self.validate_completion()?;
                self.terminal = Some(ResearchControlTerminal::Completed);
            }
            ResearchControlEvent::ResearchFailed(failure) => {
                if self.investigation.status() != InvestigationStatus::AwaitingNextStep {
                    return Err(ResearchControlTransitionError::ResearchWorkRemaining);
                }
                self.terminal = Some(ResearchControlTerminal::Failed(failure));
            }
        }
        self.last_sequence = sequence;
        Ok(())
    }

    fn apply_investigation(
        &mut self,
        outer_sequence: u64,
        record: InvestigationRecord,
    ) -> Result<(), ResearchControlTransitionError> {
        if matches!(record.event(), InvestigationEvent::FollowUpRecorded(_)) {
            return Err(ResearchControlTransitionError::UnlinkedFollowUp);
        }
        let completed_task = match record.event() {
            InvestigationEvent::TaskCompleted { task_id, .. } => Some(*task_id),
            _ => None,
        };
        let mut investigation = self.investigation.clone();
        investigation
            .apply(record)
            .map_err(ResearchControlTransitionError::Investigation)?;
        self.investigation = investigation;
        if let Some(task_id) = completed_task {
            self.task_completion_sequences
                .insert(task_id, outer_sequence);
        }
        Ok(())
    }

    fn apply_verification(
        &mut self,
        outer_sequence: u64,
        record: crate::VerificationRecord,
    ) -> Result<(), ResearchControlTransitionError> {
        let verification_id = *record.assessment().id();
        let mut verification = self.verification.clone();
        verification
            .apply(self.investigation.research(), record)
            .map_err(ResearchControlTransitionError::Verification)?;
        self.verification = verification;
        self.verification_sequences
            .insert(verification_id, outer_sequence);
        Ok(())
    }

    fn identify_gap(
        &mut self,
        sequence: u64,
        gap: IdentifiedResearchGap,
    ) -> Result<(), ResearchControlTransitionError> {
        if self.gaps.contains_key(gap.id()) {
            return Err(ResearchControlTransitionError::DuplicateGap(*gap.id()));
        }
        if self
            .gaps
            .values()
            .any(|existing| existing.gap.cause() == gap.cause())
        {
            return Err(ResearchControlTransitionError::DuplicateGapCause);
        }
        self.validate_gap_cause(gap.cause())?;
        self.gaps.insert(
            *gap.id(),
            ResearchGapState {
                gap,
                identified_sequence: sequence,
                follow_up_task_id: None,
                status: ResearchGapStatus::Open,
            },
        );
        Ok(())
    }

    fn validate_gap_cause(
        &self,
        cause: &ResearchGapCause,
    ) -> Result<(), ResearchControlTransitionError> {
        match cause {
            ResearchGapCause::Verification(id) => {
                let assessment = self.verification.assessment(id).ok_or(
                    ResearchControlTransitionError::UnknownVerificationCause(*id),
                )?;
                if is_control_ready(assessment) {
                    return Err(
                        ResearchControlTransitionError::VerificationDoesNotRequireFollowUp(*id),
                    );
                }
            }
            ResearchGapCause::InvestigationFailure(id) => {
                let task = self.investigation.task(id).ok_or(
                    ResearchControlTransitionError::UnknownInvestigationCause(*id),
                )?;
                if !matches!(task.status(), InvestigationTaskStatus::Failed(_)) {
                    return Err(ResearchControlTransitionError::InvestigationTaskNotFailed(
                        *id,
                    ));
                }
            }
        }
        Ok(())
    }

    fn record_gap_follow_up(
        &mut self,
        gap_id: ResearchGapId,
        record: InvestigationRecord,
    ) -> Result<(), ResearchControlTransitionError> {
        let task = match record.event() {
            InvestigationEvent::FollowUpRecorded(task) => task.clone(),
            _ => return Err(ResearchControlTransitionError::GapFollowUpRecordRequired),
        };
        let gap_state = self
            .gaps
            .get(&gap_id)
            .ok_or(ResearchControlTransitionError::UnknownGap(gap_id))?;
        if !matches!(gap_state.status, ResearchGapStatus::Open) {
            return Err(ResearchControlTransitionError::GapAlreadyResolved(gap_id));
        }
        if gap_state.follow_up_task_id.is_some() {
            return Err(ResearchControlTransitionError::GapAlreadyHasFollowUp(
                gap_id,
            ));
        }
        if let ResearchGapCause::InvestigationFailure(task_id) = gap_state.gap.cause() {
            return Err(ResearchControlTransitionError::InvestigationFailureCannotReplan(*task_id));
        }
        let limit = self
            .limits
            .expect("limits are checked before event dispatch")
            .max_follow_up_tasks();
        if self.follow_up_count >= limit {
            return Err(ResearchControlTransitionError::FollowUpLimitReached { limit });
        }
        let TaskOrigin::FollowUp { gap, .. } = task.origin() else {
            return Err(ResearchControlTransitionError::GapFollowUpRecordRequired);
        };
        if gap != gap_state.gap.description() {
            return Err(ResearchControlTransitionError::FollowUpGapMismatch(gap_id));
        }

        let mut investigation = self.investigation.clone();
        investigation
            .apply(record)
            .map_err(ResearchControlTransitionError::Investigation)?;
        self.investigation = investigation;
        self.gaps
            .get_mut(&gap_id)
            .expect("validated gap remains present")
            .follow_up_task_id = Some(*task.id());
        self.follow_up_count += 1;
        Ok(())
    }

    fn resolve_gap(
        &mut self,
        gap_id: ResearchGapId,
        verification_id: VerificationId,
    ) -> Result<(), ResearchControlTransitionError> {
        let gap_state = self
            .gaps
            .get(&gap_id)
            .ok_or(ResearchControlTransitionError::UnknownGap(gap_id))?
            .clone();
        if !matches!(gap_state.status, ResearchGapStatus::Open) {
            return Err(ResearchControlTransitionError::GapAlreadyResolved(gap_id));
        }
        let follow_up_id = gap_state
            .follow_up_task_id
            .ok_or(ResearchControlTransitionError::GapHasNoFollowUp(gap_id))?;
        let follow_up_sequence = self
            .task_completion_sequences
            .get(&follow_up_id)
            .copied()
            .ok_or(ResearchControlTransitionError::FollowUpNotCompleted(
                follow_up_id,
            ))?;
        if follow_up_sequence <= gap_state.identified_sequence {
            return Err(ResearchControlTransitionError::FollowUpNotCompleted(
                follow_up_id,
            ));
        }

        match gap_state.gap.cause() {
            ResearchGapCause::Verification(cause_id) => {
                let cause = self
                    .verification
                    .assessment(cause_id)
                    .expect("gap cause was validated at admission");
                let resolved = self.verification.assessment(&verification_id).ok_or(
                    ResearchControlTransitionError::UnknownResolutionVerification(verification_id),
                )?;
                let verification_sequence = self
                    .verification_sequences
                    .get(&verification_id)
                    .copied()
                    .expect("accepted verification retains its outer sequence");
                if verification_sequence <= follow_up_sequence {
                    return Err(ResearchControlTransitionError::ResolutionPrecedesFollowUp(
                        verification_id,
                    ));
                }
                if resolved.claim_id() != cause.claim_id() {
                    return Err(ResearchControlTransitionError::ResolutionClaimMismatch {
                        expected: *cause.claim_id(),
                        actual: *resolved.claim_id(),
                    });
                }
                if !is_control_ready(resolved) {
                    return Err(
                        ResearchControlTransitionError::VerificationDoesNotResolveGap(
                            verification_id,
                        ),
                    );
                }
            }
            ResearchGapCause::InvestigationFailure(task_id) => {
                return Err(
                    ResearchControlTransitionError::InvestigationFailureCannotReplan(*task_id),
                );
            }
        }

        self.gaps
            .get_mut(&gap_id)
            .expect("validated gap remains present")
            .status = ResearchGapStatus::Resolved(verification_id);
        Ok(())
    }

    fn validate_completion(&self) -> Result<(), ResearchControlTransitionError> {
        if self.investigation.status() != InvestigationStatus::AwaitingNextStep {
            return Err(ResearchControlTransitionError::ResearchWorkRemaining);
        }
        let claims = self.investigation.research().claims().collect::<Vec<_>>();
        if claims.is_empty() {
            return Err(ResearchControlTransitionError::NoClaims);
        }
        for claim in claims {
            if !self.verification.assessments().any(|assessment| {
                assessment.claim_id() == claim.id() && is_control_ready(assessment)
            }) {
                return Err(ResearchControlTransitionError::ClaimNeedsAssessment(
                    *claim.id(),
                ));
            }
        }
        for assessment in self.verification.assessments() {
            if !is_control_ready(assessment)
                && self
                    .gap_for_cause(&ResearchGapCause::Verification(*assessment.id()))
                    .is_none()
            {
                return Err(ResearchControlTransitionError::VerificationNeedsGap(
                    *assessment.id(),
                ));
            }
        }
        for task in self.investigation.tasks() {
            if matches!(task.status(), InvestigationTaskStatus::Failed(_)) {
                return Err(
                    ResearchControlTransitionError::InvestigationFailurePreventsCompletion(
                        *task.task().id(),
                    ),
                );
            }
        }
        if let Some(gap) = self
            .gaps
            .values()
            .find(|gap| matches!(gap.status, ResearchGapStatus::Open))
        {
            return Err(ResearchControlTransitionError::OpenGap(*gap.gap.id()));
        }
        Ok(())
    }

    fn gap_for_cause(&self, cause: &ResearchGapCause) -> Option<&ResearchGapState> {
        self.gaps
            .values()
            .find(|candidate| candidate.gap.cause() == cause)
    }

    fn is_terminal(&self) -> bool {
        self.terminal.is_some()
            || matches!(self.investigation.status(), InvestigationStatus::Stopped(_))
    }

    pub const fn last_sequence(&self) -> u64 {
        self.last_sequence
    }

    pub const fn limits(&self) -> Option<&ResearchControlLimits> {
        self.limits.as_ref()
    }

    pub const fn investigation(&self) -> &InvestigationState {
        &self.investigation
    }

    pub const fn verification(&self) -> &VerificationState {
        &self.verification
    }

    pub fn gap(&self, id: &ResearchGapId) -> Option<&ResearchGapState> {
        self.gaps.get(id)
    }

    pub fn gaps(&self) -> impl Iterator<Item = &ResearchGapState> {
        self.gaps.values()
    }

    pub const fn follow_up_count(&self) -> u32 {
        self.follow_up_count
    }

    pub fn status(&self) -> ResearchControlStatus {
        match &self.terminal {
            Some(ResearchControlTerminal::Completed) => ResearchControlStatus::Completed,
            Some(ResearchControlTerminal::Failed(reason)) => {
                ResearchControlStatus::Failed(reason.clone())
            }
            None => match self.investigation.status() {
                InvestigationStatus::Stopped(reason) => ResearchControlStatus::Stopped(reason),
                _ if self.limits.is_none() => ResearchControlStatus::AwaitingLimits,
                InvestigationStatus::Investigating => ResearchControlStatus::Researching,
                _ => ResearchControlStatus::AwaitingNextStep,
            },
        }
    }
}

fn is_control_ready(assessment: &VerificationAssessment) -> bool {
    let mut supports = false;
    let mut contradicts = false;
    for (_, relation) in assessment.evidence_relations() {
        match relation {
            EvidenceRelation::Supports => supports = true,
            EvidenceRelation::Contradicts => contradicts = true,
            EvidenceRelation::Unclear | EvidenceRelation::Irrelevant => {}
        }
    }
    assessment.sufficiency() == EvidenceSufficiency::Sufficient
        && (supports || contradicts)
        && !(supports && contradicts)
}
