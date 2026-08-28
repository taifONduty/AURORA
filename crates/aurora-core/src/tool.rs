use std::{collections::BTreeMap, future::Future, pin::Pin};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::{ActivitySignal, ToolEffect};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub effect: ToolEffect,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("tool arguments are invalid")]
pub struct ValidationError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolBodyResult {
    Success(serde_json::Value),
    Failed,
    Cancelled,
}

pub type ToolFuture = Pin<Box<dyn Future<Output = ToolBodyResult> + Send + 'static>>;

pub trait Tool: Send + std::fmt::Debug {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> serde_json::Value;
    fn effect(&self) -> ToolEffect;
    fn validate(&self, arguments: &serde_json::Value) -> Result<(), ValidationError>;
    /// Creates the owned execution future without doing blocking work.
    /// The tool body belongs inside the returned future so the driver can
    /// supervise its deadline, cancellation, and shutdown.
    fn execute(
        &mut self,
        arguments: serde_json::Value,
        cancellation: CancellationToken,
    ) -> ToolFuture;
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("tool definition has an empty name")]
    EmptyName,
    #[error("tool {0} has an empty description")]
    EmptyDescription(String),
    #[error("tool {0} has a non-object input schema")]
    NonObjectInputSchema(String),
    #[error("tool name {0} is registered more than once")]
    DuplicateName(String),
}

#[derive(Debug)]
pub struct ToolCatalog {
    tools: BTreeMap<String, RegisteredTool>,
}

#[derive(Debug)]
struct RegisteredTool {
    definition: ToolDefinition,
    tool: Box<dyn Tool>,
}

impl ToolCatalog {
    pub fn new(tools: Vec<Box<dyn Tool>>) -> Result<Self, CatalogError> {
        let mut registered = BTreeMap::new();
        for tool in tools {
            let name = tool.name().to_owned();
            if name.is_empty() {
                return Err(CatalogError::EmptyName);
            }
            let description = tool.description().to_owned();
            if description.is_empty() {
                return Err(CatalogError::EmptyDescription(name));
            }
            let input_schema = tool.input_schema();
            if !input_schema.is_object() {
                return Err(CatalogError::NonObjectInputSchema(name));
            }
            let definition = ToolDefinition {
                name: name.clone(),
                description,
                input_schema,
                effect: tool.effect(),
            };
            if registered
                .insert(name.clone(), RegisteredTool { definition, tool })
                .is_some()
            {
                return Err(CatalogError::DuplicateName(name));
            }
        }
        Ok(Self { tools: registered })
    }

    pub fn empty() -> Self {
        Self {
            tools: BTreeMap::new(),
        }
    }

    pub(crate) fn get_mut(&mut self, name: &str) -> Option<&mut (dyn Tool + '_)> {
        match self.tools.get_mut(name) {
            Some(registered) => Some(registered.tool.as_mut()),
            None => None,
        }
    }

    pub(crate) fn effect(&self, name: &str) -> Option<ToolEffect> {
        self.tools
            .get(name)
            .map(|registered| registered.definition.effect)
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|registered| registered.definition.clone())
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FixtureToolBehavior {
    Success(serde_json::Value),
    OrdinaryFailure,
    Panic,
    WaitForCancellation,
    IgnoreCancellation,
}

#[derive(Debug)]
pub struct FixtureTool {
    name: String,
    behavior: FixtureToolBehavior,
    activity: ActivitySignal,
}

impl FixtureTool {
    pub fn new(name: impl Into<String>, behavior: FixtureToolBehavior) -> Self {
        Self {
            name: name.into(),
            behavior,
            activity: ActivitySignal::default(),
        }
    }

    pub fn activity_signal(&self) -> ActivitySignal {
        self.activity.clone()
    }
}

impl Tool for FixtureTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        "Read one value from the fixture data."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "key": {"type": "string"}
            },
            "required": ["key"]
        })
    }

    fn effect(&self) -> ToolEffect {
        ToolEffect::ReadOnly
    }

    fn validate(&self, arguments: &serde_json::Value) -> Result<(), ValidationError> {
        if arguments
            .as_object()
            .and_then(|object| object.get("key"))
            .and_then(serde_json::Value::as_str)
            .is_some()
        {
            Ok(())
        } else {
            Err(ValidationError)
        }
    }

    fn execute(
        &mut self,
        _arguments: serde_json::Value,
        cancellation: CancellationToken,
    ) -> ToolFuture {
        let activity = self.activity.clone();
        match self.behavior.clone() {
            FixtureToolBehavior::Success(value) => Box::pin(async move {
                let _activity = activity.start_guard();
                ToolBodyResult::Success(value)
            }),
            FixtureToolBehavior::OrdinaryFailure => Box::pin(async move {
                let _activity = activity.start_guard();
                ToolBodyResult::Failed
            }),
            FixtureToolBehavior::Panic => Box::pin(async move {
                let _activity = activity.start_guard();
                panic!("fixture tool child panic")
            }),
            FixtureToolBehavior::WaitForCancellation => Box::pin(async move {
                let _activity = activity.start_guard();
                cancellation.cancelled().await;
                ToolBodyResult::Cancelled
            }),
            FixtureToolBehavior::IgnoreCancellation => Box::pin(async move {
                let _activity = activity.start_guard();
                std::future::pending::<ToolBodyResult>().await
            }),
        }
    }
}
