//! What each scheduled test protects, in closed vocabularies: the iii workers,
//! one product area and the kinds of software the subject writes. Tags select
//! tests on demand (deploy impact, a workers PR, a manual run) and feed the
//! coverage view; scheduled suites stay fixed lists. Like `protects`, tags stay
//! out of `behavior_sha256`, so retagging never makes Release Control drop a
//! test as a changed definition.
//!
//! A worker counts when the subject calls its functions (by prefix: `state::`
//! is state, `shell::`/`coder::` is ide, the engine's built-in triggers, timers
//! and configuration are engine, and so is discovery when the task depends on
//! it, as in finding a replaced tool; looking a function up before calling it
//! does not count) or when it hosts what the subject delivers (compose when the
//! app runs under iii compose, ade when the deliverable is a Console page).
//! The one exception is memory: the subject is denied its functions, and the
//! tests measure what it injects into the subject's turns unasked.
//! What only the evaluator uses to measure (the browser behind screenshots,
//! compose in cleanup) does not count. The harness is never a tag: every test
//! exercises it, and a test with no worker protects the harness alone.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::scenarios::ScenarioId;

macro_rules! vocabulary {
    ($(#[$meta:meta])* $name:ident { $($variant:ident = $id:literal,)+ }) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
        )]
        pub enum $name {
            $(#[serde(rename = $id)] $variant,)+
        }

        impl $name {
            pub const ALL: &'static [Self] = &[$(Self::$variant,)+];

            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $id,)+
                }
            }
        }
    };
}

vocabulary! {
    /// A product worker of the stack, named as in the Registry. `engine` is
    /// the exception: it groups the engine's built-in functions and follows
    /// iii releases. harness, harness-e2e, fp and provider-* are left out.
    Worker {
        Ade = "ade",
        Auth = "auth",
        Browser = "browser",
        Canvas = "canvas",
        Compose = "compose",
        ContextManager = "context-manager",
        Cron = "cron",
        Database = "database",
        Engine = "engine",
        Http = "http",
        Ide = "ide",
        IiiDirectory = "iii-directory",
        Judge = "judge",
        LlmRouter = "llm-router",
        Memory = "memory",
        Pubsub = "pubsub",
        Queue = "queue",
        RbacProxy = "rbac-proxy",
        Secrets = "secrets",
        SessionManager = "session-manager",
        State = "state",
        Storage = "storage",
        Stream = "stream",
    }
}

vocabulary! {
    /// The product capability a test protects; exactly one per test, so a
    /// report by capability never counts a test twice.
    Area {
        CoreLoop = "core-loop",
        DataSafety = "data-safety",
        ToolAdaptation = "tool-adaptation",
        Scheduling = "scheduling",
        Coding = "coding",
        Coordination = "coordination",
        SelfRepair = "self-repair",
        LongContext = "long-context",
        Security = "security",
        AppBuilding = "app-building",
        Verification = "verification",
        Memory = "memory",
    }
}

vocabulary! {
    /// The kind of software the subject delivers: a question about the model
    /// and the agent, not about iii. `testing` means the subject writes or
    /// fixes tests (running a suite does not count); `data` means the
    /// deliverable carries a schema, a migration or a persisted data model
    /// (ad hoc SQL to finish the task does not count).
    Discipline {
        Frontend = "frontend",
        Backend = "backend",
        Data = "data",
        Infra = "infra",
        Testing = "testing",
    }
}

impl Area {
    pub const fn label(self) -> &'static str {
        match self {
            Self::CoreLoop => "Basic agent loop",
            Self::DataSafety => "Safe data changes",
            Self::ToolAdaptation => "Adapting to changed tools",
            Self::Scheduling => "Timers and resumption",
            Self::Coding => "Code changes in a workspace",
            Self::Coordination => "Agents working together",
            Self::SelfRepair => "Fixing work from feedback",
            Self::LongContext => "Long sessions",
            Self::Security => "Resisting untrusted input",
            Self::AppBuilding => "Building applications",
            Self::Verification => "Verifying a change",
            Self::Memory => "Memory injection",
        }
    }
}

impl Discipline {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Frontend => "Frontend",
            Self::Backend => "Backend",
            Self::Data => "Data",
            Self::Infra => "Infra",
            Self::Testing => "Testing",
        }
    }
}

// The tags of one test as results, the catalog and the Console carry them.
// Every field is optional on the wire: an older result has none, and an
// untagged test has no area. A plain comment, not a doc comment: schemars
// would copy it onto every struct that flattens this one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScenarioTags {
    /// The iii workers the test protects; empty when it protects the harness
    /// alone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workers: Vec<Worker>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area: Option<Area>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disciplines: Vec<Discipline>,
}

impl ScenarioId {
    /// The tags of a test some suite runs; empty for the others. Values are
    /// listed in declaration order.
    pub fn tags(self) -> ScenarioTags {
        use Area as A;
        use Discipline as D;
        use Worker as W;
        let (workers, area, disciplines): (&'static [Worker], Area, &'static [Discipline]) =
            match self {
                Self::MinimalPath => (&[W::State], A::CoreLoop, &[]),
                Self::PersistentState => (&[W::State], A::DataSafety, &[]),
                Self::ToolContractRecovery => (&[W::Engine], A::ToolAdaptation, &[]),
                Self::ShellCoderSandbox => (&[W::Ide], A::Coding, &[D::Backend]),
                Self::TimerWake => (&[W::Engine, W::State], A::Scheduling, &[]),
                Self::DatabaseMigrationRecovery => (&[W::Database], A::DataSafety, &[D::Data]),
                Self::ContentionLedger => {
                    (&[W::Database, W::Engine, W::State], A::Coordination, &[])
                }
                Self::ValidationSelfRepair => (&[W::Database, W::Engine], A::SelfRepair, &[]),
                // The spawned child is the harness's; the validator, the
                // wake and the goal table are the workers'.
                Self::SubagentValidationFailure => {
                    (&[W::Database, W::Engine], A::Coordination, &[])
                }
                // A state call binding and a wake on its write: no model turn
                // between the source write and the mirror.
                Self::MechanicalReaction => (&[W::Engine, W::State], A::Scheduling, &[]),
                Self::ContextPressure => (&[], A::LongContext, &[]),
                // state only holds the canary the evaluator reads: obeying the
                // injected instruction is the failure, not a call to protect.
                Self::PromptInjectionResilience => (&[], A::Security, &[]),
                Self::RegistryImplementation => {
                    (&[], A::Coding, &[D::Frontend, D::Backend, D::Testing])
                }
                // The subject checks the delivery by hand and reports; it
                // writes no test code.
                Self::RegistryVerification => (&[], A::Verification, &[]),
                Self::TrendingTopicsBuild => (&[], A::AppBuilding, &[D::Frontend]),
                Self::AlertmanagerRouteMatch => (&[W::Ide], A::Coding, &[D::Backend]),
                // Unlike the other Console builds, the subject writes its own
                // worker-compose.yaml.
                Self::ChessEngineBuild => (
                    &[W::Ade, W::Compose, W::Ide],
                    A::AppBuilding,
                    &[D::Frontend, D::Backend, D::Infra],
                ),
                Self::FormFlowBuild | Self::StateMachineCanvasBuild => (
                    &[W::Ade, W::Canvas, W::Compose, W::Ide],
                    A::AppBuilding,
                    &[D::Frontend, D::Backend, D::Testing],
                ),
                Self::KanbanC1Foundation => (
                    &[W::Compose, W::Engine],
                    A::AppBuilding,
                    &[D::Frontend, D::Backend, D::Infra, D::Testing],
                ),
                // The ticket store, with no board UI yet.
                Self::KanbanC2Persistence => (
                    &[W::Compose, W::Engine],
                    A::AppBuilding,
                    &[D::Backend, D::Data, D::Testing],
                ),
                // c4 adds `deleted_at` to the stored ticket, c6 its comments.
                Self::KanbanC4TicketFlow | Self::KanbanC6Discussion => (
                    &[W::Compose, W::Engine],
                    A::AppBuilding,
                    &[D::Frontend, D::Backend, D::Data, D::Testing],
                ),
                Self::KanbanC3Board | Self::KanbanC5EditMove | Self::KanbanC7Live => (
                    &[W::Compose, W::Engine],
                    A::AppBuilding,
                    &[D::Frontend, D::Backend, D::Testing],
                ),
                Self::LinklyTutorial => (
                    &[
                        W::Auth,
                        W::Compose,
                        W::Database,
                        W::Engine,
                        W::Http,
                        W::Ide,
                        W::Pubsub,
                        W::Queue,
                        W::State,
                        W::Stream,
                    ],
                    A::AppBuilding,
                    &[D::Frontend, D::Backend, D::Data, D::Infra],
                ),
                Self::OnboardingTour => (
                    &[W::Ade, W::Compose, W::Database, W::Engine, W::Ide, W::State],
                    A::AppBuilding,
                    &[D::Frontend, D::Backend, D::Data],
                ),
                // The subject is denied memory's functions: what the test
                // measures is what memory injects into its turns unasked.
                Self::MemoryRelevantRecall
                | Self::MemorySmallTalk
                | Self::MemoryFirstPersonOffTopic
                | Self::MemoryParaphrase
                | Self::MemorySessionRepeat
                | Self::MemoryUpdatedFact
                | Self::MemoryValueProject
                | Self::MemoryValueProjectBaseline
                | Self::MemoryValuePreference
                | Self::MemoryValuePreferenceBaseline
                | Self::MemoryRuleFit
                | Self::MemoryChatterAfterAnswer
                | Self::MemoryUserFactNeeded
                | Self::MemoryAboutMe
                | Self::MemoryRewordedAcrossTurns
                | Self::MemorySiblingFacts
                | Self::MemoryRuleChangedMidSession
                | Self::MemoryBankEditsMidSession
                | Self::MemoryBankTrashedMidSession
                | Self::MemoryLearnThenRecall
                | Self::MemoryLearnRuleThenObey
                | Self::MemoryLearnCorrection
                | Self::MemoryAutomationNotLearned
                | Self::MemoryManySiblings
                | Self::MemoryAboutMeOverflow
                | Self::MemoryLinkedUpdate
                | Self::MemoryLearnFromLongMessage
                | Self::MemoryHonorsDoNotKeep
                | Self::MemoryKeepsWithoutARule => (&[W::Memory], A::Memory, &[]),
                _ => return ScenarioTags::default(),
            };
        ScenarioTags {
            workers: workers.to_vec(),
            area: Some(area),
            disciplines: disciplines.to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn scheduled() -> Vec<ScenarioId> {
        let plan = crate::test_plan::embedded().unwrap();
        let ids = plan.scheduled().unwrap();
        ids.iter().map(|id| id.parse().unwrap()).collect()
    }

    #[test]
    fn every_scheduled_test_is_tagged_in_order() {
        for id in scheduled() {
            let tags = id.tags();
            assert!(tags.area.is_some(), "{id} runs in a suite and has no area");
            assert!(tags.workers.is_sorted_by(|a, b| a < b), "{id} workers");
            assert!(
                tags.disciplines.is_sorted_by(|a, b| a < b),
                "{id} disciplines"
            );
        }
    }

    #[test]
    fn only_the_harness_only_tests_have_no_worker() {
        let harness_only = scheduled()
            .into_iter()
            .filter(|id| id.tags().workers.is_empty())
            .map(ScenarioId::as_str)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            harness_only,
            BTreeSet::from([
                "context_pressure",
                "prompt_injection_resilience",
                "registry_implementation",
                "registry_verification",
                "trending_topics_build",
            ])
        );
    }
}
