use assistants_core::models::Function;
use assistants_extra::llm::llm;
use async_openai::types::ChatCompletionFunctions;
use async_openai::types::FunctionCall;
use log::error;
use log::info;
use serde_json::json;
use serde_json::to_value;
use serde_json::Value;
use sqlx::types::Uuid;
use sqlx::PgPool;
use std::fmt;
use std::io::ErrorKind;
use std::{collections::HashMap, error::Error, pin::Pin};

use crate::models::FunctionCallInput;
#[derive(Clone, Debug)]
pub struct ModelConfig {
    pub model_name: String,
    pub model_url: Option<String>,
    pub user_prompt: String,
    pub temperature: Option<f32>,
    pub max_tokens_to_sample: i32,
    pub stop_sequences: Option<Vec<String>>,
    pub top_p: Option<f32>,
    pub top_k: Option<i32>,
    pub metadata: Option<HashMap<String, String>>,
}

impl ModelConfig {
    pub fn new(
        model_name: String,
        model_url: Option<String>,
        user_prompt: String,
        temperature: Option<f32>,
        max_tokens_to_sample: i32,
        stop_sequences: Option<Vec<String>>,
        top_p: Option<f32>,
        top_k: Option<i32>,
        metadata: Option<HashMap<String, String>>,
    ) -> Self {
        Self {
            model_name,
            model_url,
            user_prompt,
            temperature,
            max_tokens_to_sample,
            stop_sequences,
            top_p,
            top_k,
            metadata,
        }
    }
}

pub async fn register_function(
    pool: &PgPool,
    function: Function,
) -> Result<String, FunctionCallError> {
    let parameters_json = to_value(&function.inner.parameters).map_err(|e| {
        FunctionCallError::Other(format!("Failed to convert parameters to JSON: {}", e))
    })?;
    let user_id = Uuid::parse_str(&function.user_id)
        .map_err(|e| FunctionCallError::Other(format!("Failed to parse user_id: {}", e)))?;
    let assistant_id = Uuid::parse_str(&function.assistant_id)
        .map_err(|e| FunctionCallError::Other(format!("Failed to parse assistant_id: {}", e)))?;
    let row = sqlx::query!(
        r#"
        INSERT INTO functions (assistant_id, user_id, name, description, parameters)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id
        "#,
        assistant_id,
        user_id,
        function.inner.name,
        function.inner.description,
        &parameters_json,
    )
    .fetch_one(pool)
    .await
    .map_err(|e| FunctionCallError::SqlxError(e))?;

    Ok(row.id.clone().to_string())
}

const CREATE_FUNCTION_CALL_SYSTEM: &str = "Given the user's problem, we have a set of functions available that could potentially help solve this problem. Please review the functions and their descriptions, and select the most appropriate function to use. Also, determine the best parameters to use for this function based on the user's context. 

Please provide the name of the function you want to use and the arguments in the following format: { 'name': 'function_name', 'arguments': { 'arg_name1': 'parameter_value', 'arg_name2': 'arg_value' ... } }.

Rules:
- The function name must be one of the functions available.
- The arguments must be a subset of the arguments available.
- The arguments must be in the correct format (e.g. string, integer, etc.).
- The arguments must be required by the function (e.g. if the function requires a parameter called 'city', then you must provide a value for 'city').
- The arguments must be valid (e.g. if the function requires a parameter called 'city', then you must provide a valid city name).
- **IMPORTANT**: Your response should not be a repetition of the prompt. It should be a unique and valid function call based on the user's context and the available functions.
- If the function has no arguments, you don't need to provide the function arguments (e.g. { \"name\": \"function_name\" }).
- CUT THE FUCKING BULLSHIT - YOUR ANSWER IS JSON NOTHING ELSE
- **IMPORTANT**: IF YOU DO NOT RETURN ONLY JSON A HUMAN WILL DIE
- IF YOU USE SINGLE QUOTE INSTEAD OF DOUBLE QUOTE IN THE JSON, THE UNIVERSE WILL COME TO AN END

Examples:

1. Fetching a user's profile

Prompt:
{\"function\": {\"description\": \"Fetch a user's profile\",\"name\": \"get_user_profile\",\"parameters\": {\"username\": {\"properties\": {},\"required\": [\"username\"],\"type\": \"string\"}}},\"user_context\": \"I want to see the profile of user 'john_doe'.\"}
Answer:
{ \"name\": \"get_user_profile\", \"arguments\": { \"username\": \"john_doe\" } }

2. Sending a message

Prompt:
{\"function\": {\"description\": \"Send a message to a user\",\"name\": \"send_message\",\"parameters\": {\"recipient\": {\"properties\": {},\"required\": [\"recipient\"],\"type\": \"string\"}, \"message\": {\"properties\": {},\"required\": [\"message\"],\"type\": \"string\"}}},\"user_context\": \"I want to send 'Hello, how are you?' to 'jane_doe'.\"}
Answer:
{ \"name\": \"send_message\", \"arguments\": { \"recipient\": \"jane_doe\", \"message\": \"Hello, how are you?\" } }

Negative examples:

Prompt:
{\"function\": {\"description\": \"Get the weather for a city\",\"name\": \"weather\",\"parameters\": {\"city\": {\"properties\": {},\"required\": [\"city\"],\"type\": \"string\"}}},\"user_context\": \"Give me a weather report for Toronto, Canada.\"}
Incorrect Answer:
{ \"name\": \"weather\", \"arguments\": { \"city\": \"Toronto, Canada\" } }

In this case, the function weather expects a city parameter, but the llm provided a city and country (\"Toronto, Canada\") instead of just the city (\"Toronto\"). This would cause the function call to fail because the weather function does not know how to handle a city and country as input.


Prompt:
{\"function\": {\"description\": \"Send a message to a user\",\"name\": \"send_message\",\"parameters\": {\"recipient\": {\"properties\": {},\"required\": [\"recipient\"],\"type\": \"string\"}, \"message\": {\"properties\": {},\"required\": [\"message\"],\"type\": \"string\"}}},\"user_context\": \"I want to send 'Hello, how are you?' to 'jane_doe'.\"}
Incorrect Answer:
{\"function\": {\"description\": \"Send a message to a user\",\"name\": \"send_message\",\"parameters\": {\"recipient\": {\"properties\": {},\"required\": [\"recipient\"],\"type\": \"string\"}, \"message\": {\"properties\": {},\"required\": [\"message\"],\"type\": \"string\"}}},\"user_context\": \"I want to send 'Hello, how are you?' to 'jane_doe'.\"}

In this case, the LLM simply returned the exact same input as output, which is not a valid function call.

Your answer will be used to call the function so it must be in JSON format, do not say anything but the function name and the parameters.";

#[derive(Debug)]
pub enum FunctionCallError {
    JsonError(serde_json::Error),
    SqlxError(sqlx::Error),
    Other(String),
}

impl fmt::Display for FunctionCallError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            FunctionCallError::JsonError(e) => write!(f, "JSON error: {}", e),
            FunctionCallError::SqlxError(e) => write!(f, "SQLx error: {}", e),
            FunctionCallError::Other(e) => write!(f, "Other error: {}", e),
        }
    }
}

impl std::error::Error for FunctionCallError {}

// Pure function to generate a function call
pub async fn generate_function_call(
    input: FunctionCallInput,
) -> Result<FunctionCall, FunctionCallError> {
    let prompt_data = serde_json::json!({
        "function": {
            "name": input.function.inner.name,
            "description": input.function.inner.description,
            "parameters": input.function.inner.parameters
        },
        "user_context": input.user_context,
    });

    let prompt = match serde_json::to_string_pretty(&prompt_data) {
        Ok(json_string) => json_string,
        Err(e) => {
            error!("Failed to convert to JSON: {}", e);
            return Err(FunctionCallError::JsonError(e));
        }
    };
    info!("Generating function call with prompt: {}", prompt);
    let result = match llm(
        &input.model_config.model_name,
        input.model_config.model_url.clone(),
        CREATE_FUNCTION_CALL_SYSTEM,
        &prompt,
        input.model_config.temperature,
        -1,
        input
            .model_config
            .stop_sequences
            .as_ref()
            .map(|v| v.clone()),
        input.model_config.top_p,
        input.model_config.top_k,
        None,
        None,
    )
    .await
    {
        Ok(res) => res,
        Err(err) => {
            error!("Failed to call llm: {}", err);
            return Err(FunctionCallError::Other(format!(
                "Failed to call llm: {}",
                err
            )));
        }
    };

    string_to_function_call(&result).map_err(|e| {
        error!("Failed to convert to JSON: {}", e);
        e
    })
}

pub fn string_to_function_call(s: &str) -> Result<FunctionCall, FunctionCallError> {
    let start = s.find('{');
    let end = s.rfind('}');

    if let (Some(start), Some(end)) = (start, end) {
        let json_str = &s[start..=end];
        let json_val: Result<Value, _> = serde_json::from_str(json_str);

        match json_val {
            Ok(json) => {
                if let Some(name) = json.get("name") {
                    let name = name.to_string();
                    let arguments = json.get("arguments").unwrap_or(&json!({})).to_string();
                    Ok(FunctionCall {
                        name: name.trim_matches('\"').to_string(),
                        arguments,
                    })
                } else {
                    Err(FunctionCallError::Other(
                        "No 'name' property found in the JSON".to_string(),
                    ))
                }
            }
            Err(e) => Err(FunctionCallError::JsonError(e)),
        }
    } else {
        Err(FunctionCallError::Other(
            "No valid JSON found in the string".to_string(),
        ))
    }
}
// Function to handle database operations
pub async fn create_function_call(
    pool: &PgPool,
    assistant_id: &str,
    user_id: &str,
    model_config: ModelConfig,
) -> Result<Vec<FunctionCall>, Box<dyn Error>> {
    let rows = sqlx::query!(
        r#"
        SELECT id, name, description, parameters
        FROM functions
        WHERE user_id::text = $1 AND assistant_id::text = $2
        "#,
        user_id,
        assistant_id
    )
    .fetch_all(pool)
    .await?;

    let mut results = Vec::new();

    for row in rows {
        let input = FunctionCallInput {
            function: Function {
                inner: ChatCompletionFunctions {
                    name: row.name.unwrap_or_default(),
                    description: row.description,
                    parameters: serde_json::from_value(row.parameters.unwrap_or_default())?,
                },
                assistant_id: assistant_id.to_string(),
                user_id: user_id.to_string(),
            },
            user_context: model_config.user_prompt.clone(),
            model_config: model_config.clone(),
        };

        let result = generate_function_call(input).await?;
        results.push(result);
    }

    Ok(results)
}
// ! TODO next: fix mistral 7b (prompt is not good enough, stupid LLM returns exactly the prompt he was given), then create list of tests to run for all cases (multiple functions, multiple parameters, different topics, etc.)

#[cfg(test)]
mod tests {
    use crate::{assistants::create_assistant, models::Assistant};

    use super::*;
    use async_openai::types::{AssistantObject, AssistantTools, AssistantToolsFunction};
    use dotenv;
    use serde_json::json;
    use sqlx::postgres::PgPoolOptions;
    use std::env;

    async fn reset_db(pool: &PgPool) {
        // TODO should also purge minio
        sqlx::query!(
            "TRUNCATE assistants, threads, messages, runs, functions, tool_calls RESTART IDENTITY"
        )
        .execute(pool)
        .await
        .unwrap();
    }
    #[tokio::test]
    #[ignore]
    async fn test_create_function_call_with_openai() {
        dotenv::dotenv().ok();
        let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&database_url)
            .await
            .expect("Failed to create pool.");
        reset_db(&pool).await;
        let user_id = Uuid::default().to_string();
        let assistant = Assistant {
            inner: AssistantObject {
                id: "".to_string(),
                instructions: Some("".to_string()),
                name: Some("Math Tutor".to_string()),
                tools: vec![],
                model: "gpt-3.5-turbo".to_string(),
                file_ids: vec![],
                object: "object_value".to_string(),
                created_at: 0,
                description: Some("description_value".to_string()),
                metadata: None,
            },
            user_id: Uuid::default().to_string(),
        };
        let assistant = create_assistant(&pool, &assistant).await.unwrap();

        // Mock weather function
        async fn weather(city: &str) -> String {
            let city = city.to_lowercase();
            if city == "toronto" {
                "The weather in Toronto is sunny.".to_string()
            } else if city == "vancouver" {
                "The weather in Vancouver is rainy.".to_string()
            } else {
                format!("The weather in {} is unknown.", city)
            }
        }

        // Register the weather function
        let weather_function = Function {
            inner: ChatCompletionFunctions {
                name: String::from("weather"),
                description: Some(String::from("Get the weather for a city")),
                parameters: json!({
                    "type": "object",
                    "required": ["city"],
                    "properties": {
                        "city": {
                            "type": "string",
                            "description": null,
                            "enum": null
                        }
                    }
                }),
            },
            assistant_id: assistant.inner.id.clone(),
            user_id: user_id.clone(),
        };
        register_function(&pool, weather_function).await.unwrap();

        let model_config = ModelConfig {
            model_name: String::from("gpt-3.5-turbo"),
            model_url: None,
            user_prompt: String::from("Give me a weather report for Toronto, Canada."),
            temperature: Some(0.5),
            max_tokens_to_sample: 200,
            stop_sequences: None,
            top_p: Some(1.0),
            top_k: None,
            metadata: None,
        };

        let result = create_function_call(&pool, &assistant.inner.id, &user_id, model_config).await;

        match result {
            Ok(function_results) => {
                for function_result in function_results {
                    let function_name = function_result.name;
                    let parameters = function_result.arguments;
                    assert_eq!(function_name, "weather");
                    let param_json: HashMap<String, String> =
                        serde_json::from_str(&parameters).unwrap();

                    // execute the function
                    let city = param_json.get("city").unwrap().to_string();
                    let weather = weather(&city).await;
                    assert_eq!(weather, "The weather in Toronto is sunny.");
                }
            }
            Err(e) => panic!("Function call failed: {:?}", e),
        }
    }

    #[tokio::test]
    #[ignore]
    async fn test_create_function_call_with_anthropic() {
        dotenv::dotenv().ok();
        let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&database_url)
            .await
            .expect("Failed to create pool.");
        reset_db(&pool).await;
        let user_id = Uuid::default().to_string();
        let assistant = Assistant {
            inner: AssistantObject {
                id: "".to_string(),
                instructions: Some("".to_string()),
                name: Some("Math Tutor".to_string()),
                tools: vec![],
                model: "mixtral-8x7b-instruct".to_string(),
                file_ids: vec![],
                object: "object_value".to_string(),
                created_at: 0,
                description: Some("description_value".to_string()),
                metadata: None,
            },
            user_id: Uuid::default().to_string(),
        };
        let assistant = create_assistant(&pool, &assistant).await.unwrap();

        // Mock weather function
        async fn weather(city: &str) -> String {
            let city = city.to_lowercase();
            if city == "toronto" {
                "The weather in Toronto is sunny.".to_string()
            } else if city == "vancouver" {
                "The weather in Vancouver is rainy.".to_string()
            } else {
                format!("The weather in {} is unknown.", city)
            }
        }

        // Register the weather function
        let weather_function = Function {
            assistant_id: assistant.inner.id.clone(),
            user_id: user_id.clone(),
            inner: ChatCompletionFunctions {
                name: String::from("weather"),
                description: Some(String::from("Get the weather for a city")),
                parameters: json!({
                    "type": "object",
                    "required": ["city"],
                    "properties": {
                        "city": {
                            "type": "string",
                            "description": null,
                            "enum": null
                        }
                    }
                }),
            },
        };
        register_function(&pool, weather_function).await.unwrap();

        let user_id = Uuid::default().to_string();
        let model_config = ModelConfig {
            model_name: String::from("claude-2.1"),
            model_url: None,
            user_prompt: String::from("Give me a weather report for Toronto, Canada."),
            temperature: Some(0.5),
            max_tokens_to_sample: 200,
            stop_sequences: None,
            top_p: Some(1.0),
            top_k: None,
            metadata: None,
        };

        let result = create_function_call(&pool, &assistant.inner.id, &user_id, model_config).await;

        match result {
            Ok(function_results) => {
                for function_result in function_results {
                    let function_name = function_result.name;
                    let parameters = function_result.arguments;
                    assert_eq!(function_name, "weather");
                    let param_json: HashMap<String, String> =
                        serde_json::from_str(&parameters).unwrap();

                    // execute the function
                    let city = param_json.get("city").unwrap().to_string();
                    let weather = weather(&city).await;
                    assert_eq!(weather, "The weather in Toronto is sunny.");
                }
            }
            Err(e) => panic!("Function call failed: {:?}", e),
        }
    }

    #[tokio::test]
    #[ignore]
    async fn test_create_function_call_with_llama_2_70b_chat() {
        dotenv::dotenv().ok();
        let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&database_url)
            .await
            .expect("Failed to create pool.");
        reset_db(&pool).await;
        let user_id = Uuid::default().to_string();
        let assistant = Assistant {
            inner: AssistantObject {
                id: "".to_string(),
                instructions: Some("".to_string()),
                name: Some("Math Tutor".to_string()),
                tools: vec![],
                model: "open-source/llama-2-70b-chat".to_string(),
                file_ids: vec![],
                object: "object_value".to_string(),
                created_at: 0,
                description: Some("description_value".to_string()),
                metadata: None,
            },
            user_id: Uuid::default().to_string(),
        };

        let assistant = create_assistant(&pool, &assistant).await.unwrap();

        // Mock weather function
        async fn weather(city: &str) -> String {
            let city = city.to_lowercase();
            if city == "toronto" {
                "The weather in Toronto is sunny.".to_string()
            } else if city == "vancouver" {
                "The weather in Vancouver is rainy.".to_string()
            } else {
                format!("The weather in {} is unknown.", city)
            }
        }

        // Register the weather function
        let weather_function = Function {
            assistant_id: assistant.inner.id.clone(),
            user_id: user_id.clone(),
            inner: ChatCompletionFunctions {
                name: String::from("weather"),
                description: Some(String::from("Get the weather for a city")),
                parameters: json!({
                    "type": "object",
                    "required": ["city"],
                    "properties": {
                        "city": {
                            "type": "string",
                            "description": null,
                            "enum": null
                        }
                    }
                }),
            },
        };
        register_function(&pool, weather_function).await.unwrap();

        let user_id = Uuid::default().to_string();
        let model_config = ModelConfig {
            // model_name: String::from("open-source/mistral-7b-instruct"),
            model_name: String::from("open-source/llama-2-70b-chat"),
            model_url: Some("https://api.perplexity.ai/chat/completions".to_string()),
            user_prompt: String::from("Give me a weather report for Toronto, Canada."),
            temperature: Some(0.0),
            max_tokens_to_sample: 200,
            stop_sequences: None,
            top_p: Some(1.0),
            top_k: None,
            metadata: None,
        };

        let result = create_function_call(&pool, &assistant.inner.id, &user_id, model_config).await;

        match result {
            Ok(function_results) => {
                for function_result in function_results {
                    let function_name = function_result.name;
                    let parameters = function_result.arguments;
                    assert_eq!(function_name, "weather");
                    let param_json: HashMap<String, String> =
                        serde_json::from_str(&parameters).unwrap();

                    // execute the function
                    let city = param_json.get("city").unwrap().to_string();
                    let weather = weather(&city).await;
                    assert_eq!(weather, "The weather in Toronto is sunny.");
                }
            }
            Err(e) => panic!("Function call failed: {:?}", e),
        }
    }

    #[tokio::test]
    #[ignore]

    async fn test_generate_function_call_with_llama_2_70b() {
        dotenv::dotenv().ok();
        let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");

        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&database_url)
            .await
            .expect("Failed to create pool.");
        let user_id = Uuid::default().to_string();
        let assistant = Assistant {
            inner: AssistantObject {
                id: "".to_string(),
                instructions: Some("".to_string()),
                name: Some("Math Tutor".to_string()),
                tools: vec![],
                model: "open-source/llama-2-70b-chat".to_string(),
                file_ids: vec![],
                object: "object_value".to_string(),
                created_at: 0,
                description: Some("description_value".to_string()),
                metadata: None,
            },
            user_id: Uuid::default().to_string(),
        };

        let assistant = create_assistant(&pool, &assistant).await.unwrap();

        let function = Function {
            assistant_id: assistant.inner.id.clone(),
            user_id: user_id.clone(),
            inner: ChatCompletionFunctions {
                name: String::from("weather"),
                description: Some(String::from("Get the weather for a city")),
                parameters: json!({
                    "type": "object",
                    "required": ["city"],
                    "properties": {
                        "city": {
                            "type": "string",
                            "description": null,
                            "enum": null
                        }
                    }
                }),
            },
        };

        let user_context = String::from("Give me a weather report for Toronto, Canada.");

        let model_config = ModelConfig {
            model_name: String::from("open-source/llama-2-70b-chat"),
            model_url: Some("https://api.perplexity.ai/chat/completions".to_string()),
            user_prompt: user_context.clone(),
            temperature: Some(0.0),
            max_tokens_to_sample: 200,
            stop_sequences: None,
            top_p: Some(1.0),
            top_k: None,
            metadata: None,
        };

        let input = FunctionCallInput {
            function,
            user_context,
            model_config,
        };

        let result = generate_function_call(input).await;

        match result {
            Ok(function_result) => {
                assert_eq!(function_result.name, "weather");
                let parameters = function_result.arguments;
                let param_json: HashMap<String, String> =
                    serde_json::from_str(&parameters).unwrap();
                assert_eq!(param_json.get("city").unwrap(), "Toronto");
            }
            Err(e) => panic!("Function call failed: {:?}", e),
        }
    }

    #[tokio::test]
    #[ignore]
    async fn test_generate_function_call_with_mixtral_8x7b() {
        dotenv::dotenv().ok();
        let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");

        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&database_url)
            .await
            .expect("Failed to create pool.");
        let user_id = Uuid::default().to_string();
        let assistant = Assistant {
            inner: AssistantObject {
                id: "".to_string(),
                instructions: Some("".to_string()),
                name: Some("Math Tutor".to_string()),
                tools: vec![],
                model: "open-source/llama-2-70b-chat".to_string(),
                file_ids: vec![],
                object: "object_value".to_string(),
                created_at: 0,
                description: Some("description_value".to_string()),
                metadata: None,
            },
            user_id: Uuid::default().to_string(),
        };

        let assistant = create_assistant(&pool, &assistant).await.unwrap();

        let function = Function {
            assistant_id: assistant.inner.id.clone(),
            user_id: user_id.clone(),
            inner: ChatCompletionFunctions {
                name: String::from("weather"),
                description: Some(String::from("Get the weather for a city")),
                parameters: json!({
                    "type": "object",
                    "required": ["city"],
                    "properties": {
                        "city": {
                            "type": "string",
                            "description": null,
                            "enum": null
                        }
                    }
                }),
            },
        };

        let user_context = String::from("Give me a weather report for Toronto, Canada.");

        let model_config = ModelConfig {
            model_name: String::from("mistralai/mixtral-8x7b-instruct"),
            model_url: Some("https://api.perplexity.ai/chat/completions".to_string()),
            user_prompt: user_context.clone(),
            temperature: Some(0.0),
            max_tokens_to_sample: 200,
            stop_sequences: None,
            top_p: Some(1.0),
            top_k: None,
            metadata: None,
        };

        let input = FunctionCallInput {
            function,
            user_context,
            model_config,
        };

        let result = generate_function_call(input).await;

        match result {
            Ok(function_result) => {
                assert_eq!(function_result.name, "weather");
                let parameters = function_result.arguments;
                let param_json: HashMap<String, String> =
                    serde_json::from_str(&parameters).unwrap();
                assert_eq!(param_json.get("city").unwrap(), "Toronto");
            }
            Err(e) => panic!("Function call failed: {:?}", e),
        }
    }

    #[test]
    #[ignore]
    fn test_string_to_function_call() {
        // Case 1: Valid JSON embedded within non-JSON content
        let input = "Some non-JSON content...\
                     {\"name\": \"calculator\", \"arguments\": {\"a\": 5, \"b\": 3}}\
                     More non-JSON content...";
        let result = string_to_function_call(input).unwrap();
        assert_eq!(result.name, "calculator");
        assert_eq!(result.arguments, "{\"a\":5,\"b\":3}");
        println!("passed case 1");
        // Case 2: String with no valid JSON
        let input = "This string has no valid JSON";
        let result = string_to_function_call(input);
        assert!(result.is_err(), "Expected error, but got {:?}", result);
        println!("passed case 2");
        // Case 3: JSON object without 'name' property
        let input = "{\"arguments\": {\"a\": 5, \"b\": 3}}";
        let result = string_to_function_call(input);
        assert!(result.is_err(), "Expected error, but got {:?}", result);
        println!("passed case 3");
        // Case 4: JSON object without 'arguments' property
        let input = "{\"name\": \"calculator\"}";
        let result = string_to_function_call(input).unwrap();
        assert_eq!(result.name, "calculator");
        assert_eq!(result.arguments, "{}");
        println!("passed case 4");
        // Case 5: JSON object with extra properties
        let input = "{\"name\": \"calculator\", \"arguments\": {\"a\": 5, \"b\": 3}, \"extra\": \"property\"}";
        let result = string_to_function_call(input).unwrap();
        assert_eq!(
            result.name, "calculator",
            "Expected name to be 'calculator', but got {}",
            result.name
        );
        assert_eq!(
            result.arguments, "{\"a\":5,\"b\":3}",
            "Expected arguments to be {{\"a\":5,\"b\":3}}, but got {}",
            result.arguments
        );
    }
}
