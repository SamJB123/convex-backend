use common::{
    bootstrap_model::components::ComponentState,
    components::{
        CanonicalizedComponentFunctionPath,
        ComponentId,
        ComponentPath,
    },
    testing::assert_contains,
    types::{
        EnvironmentVariable,
        FunctionCaller,
    },
    RequestId,
};
use database::{
    BootstrapComponentsModel,
    TableModel,
    UserFacingModel,
};
use errors::ErrorMetadataAnyhowExt;
use futures::FutureExt;
use itertools::Itertools;
use keybroker::Identity;
use must_let::must_let;
use runtime::testing::TestRuntime;
use serde_json::{
    json,
    Value as JsonValue,
};
use sync_types::CanonicalizedUdfPath;
use value::{
    assert_obj,
    ConvexObject,
    ConvexValue,
    TableName,
    TableNamespace,
};

use crate::{
    test_helpers::ApplicationTestExt,
    Application,
    FunctionError,
    FunctionReturn,
};

async fn run_function(
    application: &Application<TestRuntime>,
    udf_path: CanonicalizedUdfPath,
    args: Vec<JsonValue>,
) -> anyhow::Result<Result<FunctionReturn, FunctionError>> {
    run_component_function(application, udf_path, args, ComponentPath::root()).await
}

async fn run_component_function(
    application: &Application<TestRuntime>,
    udf_path: CanonicalizedUdfPath,
    args: Vec<JsonValue>,
    component: ComponentPath,
) -> anyhow::Result<Result<FunctionReturn, FunctionError>> {
    application
        .any_udf(
            RequestId::new(),
            CanonicalizedComponentFunctionPath {
                component,
                udf_path,
            },
            args,
            Identity::system(),
            FunctionCaller::Test,
        )
        .boxed()
        .await
}

#[convex_macro::test_runtime]
async fn test_run_component_query(rt: TestRuntime) -> anyhow::Result<()> {
    let application = Application::new_for_tests(&rt).await?;
    application
        .load_component_tests_modules("with-schema")
        .await?;
    let result = run_function(&application, "componentEntry:list".parse()?, vec![]).await??;
    assert_eq!(result.log_lines.iter().collect_vec().len(), 1);
    Ok(())
}

#[convex_macro::test_runtime]
async fn test_run_component_mutation(rt: TestRuntime) -> anyhow::Result<()> {
    let application = Application::new_for_tests(&rt).await?;
    application
        .load_component_tests_modules("with-schema")
        .await?;
    let result = run_function(
        &application,
        "componentEntry:insert".parse()?,
        vec![json!({"channel": "random", "text": "convex is kewl"})],
    )
    .await?;
    assert!(result.is_ok());
    Ok(())
}

#[convex_macro::test_runtime]
async fn test_run_component_action(rt: TestRuntime) -> anyhow::Result<()> {
    let application = Application::new_for_tests(&rt).await?;
    application
        .load_component_tests_modules("with-schema")
        .await?;
    let result = run_function(&application, "componentEntry:hello".parse()?, vec![]).await??;
    // No logs returned because only the action inside the component logs.
    assert_eq!(result.log_lines.iter().collect_vec().len(), 0);
    Ok(())
}

#[convex_macro::test_runtime]
async fn test_env_var_works_in_app_definition(rt: TestRuntime) -> anyhow::Result<()> {
    let application = Application::new_for_tests(&rt).await?;
    let mut tx = application.begin(Identity::system()).await?;
    application
        .create_one_environment_variable(
            &mut tx,
            EnvironmentVariable {
                name: "NAME".parse()?,
                value: "emma".parse()?,
            },
        )
        .await?;
    application.commit_test(tx).await?;
    application.load_component_tests_modules("basic").await?;
    let result = run_function(&application, "componentEntry:hello".parse()?, vec![]).await??;
    must_let!(let ConvexValue::String(name) = result.value);
    assert_eq!(name.to_string(), "emma".to_string());

    // No logs returned because only the action inside the component logs.
    assert_eq!(result.log_lines.iter().collect_vec().len(), 0);
    Ok(())
}

#[convex_macro::test_runtime]
async fn test_system_env_var_works_in_app_definition(rt: TestRuntime) -> anyhow::Result<()> {
    let application = Application::new_for_tests(&rt).await?;
    application.load_component_tests_modules("basic").await?;
    let result = run_function(&application, "componentEntry:url".parse()?, vec![]).await??;
    must_let!(let ConvexValue::String(name) = result.value);
    assert_eq!(name.to_string(), "http://127.0.0.1:8000".to_string());
    Ok(())
}

#[convex_macro::test_runtime]
async fn test_env_vars_not_accessible_in_components(rt: TestRuntime) -> anyhow::Result<()> {
    let application = Application::new_for_tests(&rt).await?;
    application.load_component_tests_modules("basic").await?;
    let mut tx = application.begin(Identity::system()).await?;
    application
        .create_one_environment_variable(
            &mut tx,
            EnvironmentVariable {
                name: "NAME".parse()?,
                value: "emma".parse()?,
            },
        )
        .await?;
    application.commit_test(tx).await?;
    let result =
        run_function(&application, "componentEntry:envVarQuery".parse()?, vec![]).await??;
    assert_eq!(ConvexValue::Null, result.value);
    let result =
        run_function(&application, "componentEntry:envVarAction".parse()?, vec![]).await??;
    assert_eq!(ConvexValue::Null, result.value);
    Ok(())
}

#[convex_macro::test_runtime]
async fn test_system_env_vars_not_accessible_in_components(rt: TestRuntime) -> anyhow::Result<()> {
    let application = Application::new_for_tests(&rt).await?;
    application.load_component_tests_modules("basic").await?;
    let result = run_function(
        &application,
        "componentEntry:systemEnvVarQuery".parse()?,
        vec![],
    )
    .await??;
    assert_eq!(ConvexValue::Null, result.value);
    let result = run_function(
        &application,
        "componentEntry:systemEnvVarAction".parse()?,
        vec![],
    )
    .await??;
    assert_eq!(ConvexValue::Null, result.value);
    Ok(())
}

#[convex_macro::test_runtime]
async fn test_system_error_propagation(rt: TestRuntime) -> anyhow::Result<()> {
    let application = Application::new_for_tests(&rt).await?;
    application.load_component_tests_modules("basic").await?;

    // The system error from the subquery should propagate to the top-level query.
    let error = run_function(
        &application,
        "errors:throwSystemErrorFromQuery".parse()?,
        vec![],
    )
    .await
    .unwrap_err();
    assert_contains(&error, "I can't go for that");

    // Actions throw a JS error into user space when a call to `ctx.runAction`
    // throws a system error, so we don't propagate them here.
    let result = run_function(
        &application,
        "errors:throwSystemErrorFromAction".parse()?,
        vec![],
    )
    .await?
    .unwrap_err();
    assert_contains(&result.error, "Your request couldn't be completed");

    Ok(())
}

#[convex_macro::test_runtime]
async fn test_delete_tables_in_component(rt: TestRuntime) -> anyhow::Result<()> {
    let application = Application::new_for_tests(&rt).await?;
    application.load_component_tests_modules("mounted").await?;
    let mut tx = application.begin(Identity::system()).await?;
    let mut components_model = BootstrapComponentsModel::new(&mut tx);
    let (_, component_id) = components_model
        .component_path_to_ids(component_path())
        .await?;

    // Create a table in a new namespace
    let table_namespace = TableNamespace::from(component_id);
    let mut user_facing_model = UserFacingModel::new(&mut tx, table_namespace);
    let table_name: TableName = "test".parse()?;
    user_facing_model
        .insert(table_name.clone(), assert_obj!())
        .await?;
    application.commit_test(tx).await?;

    // Confirm table exists and document is present
    let mut tx = application.begin(Identity::system()).await?;
    let mut table_model = TableModel::new(&mut tx);
    let count = table_model.count(table_namespace, &table_name).await?;
    assert_eq!(count, 1);
    assert!(table_model.table_exists(table_namespace, &table_name));

    // Delete the table
    application
        .delete_tables(
            &Identity::system(),
            vec![table_name.clone()],
            table_namespace,
        )
        .await?;

    // Confirm table no longer exists
    let mut tx = application.begin(Identity::system()).await?;
    let mut table_model = TableModel::new(&mut tx);
    assert!(!table_model.table_exists(table_namespace, &table_name));
    Ok(())
}

async fn unmount_component(application: &Application<TestRuntime>) -> anyhow::Result<ComponentId> {
    application.load_component_tests_modules("mounted").await?;
    run_component_function(
        application,
        "messages:insertMessage".parse()?,
        vec![example_message().into()],
        component_path(),
    )
    .await??;

    // Unmount component
    application.load_component_tests_modules("empty").await?;
    let mut tx = application.begin(Identity::system()).await?;
    let mut components_model = BootstrapComponentsModel::new(&mut tx);
    let (_, component_id) = components_model
        .component_path_to_ids(component_path())
        .await?;
    Ok(component_id)
}

fn example_message() -> ConvexObject {
    assert_obj!("channel" => "sports", "text" => "the celtics won!")
}

fn component_path() -> ComponentPath {
    ComponentPath::deserialize(Some("component")).unwrap()
}

fn table_name() -> TableName {
    "messages".parse().unwrap()
}

#[convex_macro::test_runtime]
async fn test_unmounted_component_state(rt: TestRuntime) -> anyhow::Result<()> {
    let application = Application::new_for_tests(&rt).await?;
    let component_id = unmount_component(&application).await?;
    let mut tx = application.begin(Identity::system()).await?;
    let mut components_model = BootstrapComponentsModel::new(&mut tx);
    let component = components_model
        .load_component(component_id)
        .await?
        .unwrap();
    assert!(matches!(component.state, ComponentState::Unmounted));

    // Remount the component
    application.load_component_tests_modules("mounted").await?;

    let mut tx = application.begin(Identity::system()).await?;
    let mut components_model = BootstrapComponentsModel::new(&mut tx);
    // Component at the same path should be remounted with the same id.
    let (_, new_component_id) = components_model
        .component_path_to_ids(component_path())
        .await?;
    assert_eq!(component_id, new_component_id);
    let component = components_model
        .load_component(component_id)
        .await?
        .unwrap();
    assert!(matches!(component.state, ComponentState::Active));
    Ok(())
}

#[convex_macro::test_runtime]
async fn test_unmount_cannot_call_functions(rt: TestRuntime) -> anyhow::Result<()> {
    let application = Application::new_for_tests(&rt).await?;
    unmount_component(&application).await?;
    // Calling component function after the component is unmounted should fail
    let result = run_component_function(
        &application,
        "messages:listMessages".parse()?,
        vec![assert_obj!().into()],
        ComponentPath::deserialize(Some("component"))?,
    )
    .await?;
    assert!(result.is_err());
    Ok(())
}

#[convex_macro::test_runtime]
async fn test_writes_to_unmounted_tables_fails(rt: TestRuntime) -> anyhow::Result<()> {
    let application = Application::new_for_tests(&rt).await?;
    let component_id = unmount_component(&application).await?;
    let mut tx = application.begin(Identity::system()).await?;
    let mut user_model = UserFacingModel::new(&mut tx, TableNamespace::from(component_id));
    let error = user_model
        .insert(table_name(), example_message())
        .await
        .unwrap_err();
    assert!(error.is_bad_request());
    assert_eq!(error.short_msg(), "UnmountedComponent");
    Ok(())
}

#[convex_macro::test_runtime]
async fn test_data_exists_in_unmounted_components(rt: TestRuntime) -> anyhow::Result<()> {
    let application = Application::new_for_tests(&rt).await?;
    let component_id = unmount_component(&application).await?;
    let mut tx = application.begin(Identity::system()).await?;
    let mut table_model = TableModel::new(&mut tx);
    let count = table_model
        .count(component_id.into(), &table_name())
        .await?;
    assert_eq!(count, 1);
    Ok(())
}

#[convex_macro::test_runtime]
async fn test_descendents_unmounted(rt: TestRuntime) -> anyhow::Result<()> {
    let application = Application::new_for_tests(&rt).await?;
    unmount_component(&application).await?;
    let mut tx = application.begin(Identity::system()).await?;
    let mut components_model = BootstrapComponentsModel::new(&mut tx);
    let env_vars_child_component = ComponentPath::deserialize(Some("envVars/component"))?;
    let (_, component_id) = components_model
        .component_path_to_ids(env_vars_child_component)
        .await?;
    let metadata = components_model
        .load_component(component_id)
        .await?
        .unwrap();
    assert!(matches!(metadata.state, ComponentState::Unmounted));
    Ok(())
}
