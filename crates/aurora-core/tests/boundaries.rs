use aurora_core::{
    ActivitySignal, FixtureTool, FixtureToolBehavior, ModelBackend, ModelInput, ModelInvocation,
    ModelItem, ScriptedModel, ScriptedModelStep, Tool, ToolCatalog, ToolEffect, ToolRequest,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

fn fixture_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "key": {"type": "string"}
        },
        "required": ["key"]
    })
}

fn model_input() -> ModelInput {
    ModelInput {
        context: vec![ModelItem::UserInput {
            text: "request".to_owned(),
        }],
        tools: vec![aurora_core::ToolDefinition {
            name: "fixture.read".to_owned(),
            description: "Read one value from the fixture data.".to_owned(),
            input_schema: fixture_schema(),
            effect: ToolEffect::ReadOnly,
        }],
    }
}

#[derive(Debug)]
struct AdvertisedTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

impl Tool for AdvertisedTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn input_schema(&self) -> serde_json::Value {
        self.input_schema.clone()
    }
    fn effect(&self) -> ToolEffect {
        ToolEffect::ReadOnly
    }
    fn validate(&self, _arguments: &serde_json::Value) -> Result<(), aurora_core::ValidationError> {
        Ok(())
    }
    fn execute(
        &mut self,
        _arguments: serde_json::Value,
        _cancellation: CancellationToken,
    ) -> aurora_core::ToolFuture {
        Box::pin(async { aurora_core::ToolBodyResult::Success(json!({})) })
    }
}

fn advertised_tool(
    name: &str,
    description: &str,
    input_schema: serde_json::Value,
) -> Box<dyn Tool> {
    Box::new(AdvertisedTool {
        name: name.to_owned(),
        description: description.to_owned(),
        input_schema,
    })
}

#[test]
fn catalog_freezes_complete_tool_definitions() {
    let catalog = ToolCatalog::new(vec![advertised_tool(
        "fixture.read",
        "Read one value from the fixture data.",
        fixture_schema(),
    )])
    .expect("definition is valid");
    assert_eq!(
        catalog.definitions(),
        [aurora_core::ToolDefinition {
            name: "fixture.read".to_owned(),
            description: "Read one value from the fixture data.".to_owned(),
            input_schema: fixture_schema(),
            effect: ToolEffect::ReadOnly,
        }]
    );
}

#[test]
fn catalog_rejects_an_empty_tool_name() {
    let error = ToolCatalog::new(vec![advertised_tool("", "Description", fixture_schema())])
        .expect_err("empty names are invalid");
    assert_eq!(error.to_string(), "tool definition has an empty name");
}

#[test]
fn catalog_rejects_an_empty_tool_description() {
    let error = ToolCatalog::new(vec![advertised_tool("fixture.read", "", fixture_schema())])
        .expect_err("empty descriptions are invalid");
    assert_eq!(
        error.to_string(),
        "tool fixture.read has an empty description"
    );
}

#[test]
fn catalog_rejects_a_non_object_input_schema() {
    let error = ToolCatalog::new(vec![advertised_tool(
        "fixture.read",
        "Description",
        json!(true),
    )])
    .expect_err("the advertised schema must be an object");
    assert_eq!(
        error.to_string(),
        "tool fixture.read has a non-object input schema"
    );
}

#[tokio::test]
async fn scripted_model_returns_steps_in_order_and_counts_invocations() {
    let mut model = ScriptedModel::new(vec![
        ScriptedModelStep::Return(ModelInvocation::ToolRequest(ToolRequest {
            tool_call_id: aurora_core::ToolCallId::new("call-1"),
            name: "fixture.read".to_owned(),
            arguments: json!({"key": "alpha"}),
        })),
        ScriptedModelStep::Return(ModelInvocation::FinalResponse {
            text: "done".to_owned(),
        }),
    ]);

    let first = model.invoke(model_input(), CancellationToken::new()).await;
    let second = model.invoke(model_input(), CancellationToken::new()).await;

    assert!(matches!(first, ModelInvocation::ToolRequest(_)));
    assert_eq!(
        second,
        ModelInvocation::FinalResponse {
            text: "done".to_owned()
        }
    );
    assert_eq!(model.invocation_count(), 2);
    assert_eq!(model.inputs(), &[model_input(), model_input()]);
}

#[tokio::test]
async fn pending_scripted_model_stops_when_cancelled() {
    let mut model = ScriptedModel::new(vec![ScriptedModelStep::WaitForCancellation]);
    let activity: ActivitySignal = model.activity_signal();
    let cancellation = CancellationToken::new();
    let future = model.invoke(model_input(), cancellation.clone());
    assert_eq!(activity.starts(), 0);
    let child = tokio::spawn(future);

    activity.wait_for_starts(1).await;
    cancellation.cancel();

    assert_eq!(
        child.await.expect("scripted model child joins"),
        ModelInvocation::Cancelled
    );
    assert_eq!(activity.starts(), 1);
    assert_eq!(activity.stops(), 1);
}

#[test]
fn duplicate_tool_registration_is_rejected() {
    let first: Box<dyn Tool> = Box::new(FixtureTool::new(
        "fixture.read",
        FixtureToolBehavior::Success(json!({"value": "first"})),
    ));
    let second: Box<dyn Tool> = Box::new(FixtureTool::new(
        "fixture.read",
        FixtureToolBehavior::Success(json!({"value": "second"})),
    ));

    let error = ToolCatalog::new(vec![first, second]).expect_err("duplicate names must fail");

    assert_eq!(
        error.to_string(),
        "tool name fixture.read is registered more than once"
    );
}

#[tokio::test]
async fn fixture_tool_validates_before_execution_and_cooperates_with_cancellation() {
    let mut tool = FixtureTool::new("fixture.read", FixtureToolBehavior::WaitForCancellation);
    let activity = tool.activity_signal();

    assert!(tool.validate(&json!({"key": "alpha"})).is_ok());
    assert!(tool.validate(&json!({"key": 3})).is_err());

    let cancellation = CancellationToken::new();
    let future = tool.execute(json!({"key": "alpha"}), cancellation.clone());
    assert_eq!(activity.starts(), 0);
    let child = tokio::spawn(future);
    activity.wait_for_starts(1).await;
    cancellation.cancel();

    assert_eq!(
        child.await.expect("fixture tool child joins"),
        aurora_core::ToolBodyResult::Cancelled
    );
    assert_eq!(activity.starts(), 1);
    assert_eq!(activity.stops(), 1);
}
