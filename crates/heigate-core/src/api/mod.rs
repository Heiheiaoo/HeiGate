//! The local management API (`/api/*`) and the gateway endpoints (`/v1/*`).

pub mod codex;
pub mod config;
pub mod forward;
pub mod gateway;
pub mod handlers;
pub mod keys;
pub mod policy;

pub use gateway::{Gateway, GatewayError, RequestContext};
pub use handlers::AppState;

pub use forward::{
    anthropic_error_type, estimate_prompt_tokens, extract_non_streaming_tokens,
    gateway_candidate_matcher, is_client_closed_conn, is_forwardable_client_header,
    is_hop_by_hop_header, parse_retry_after, request_model, GatewayProtocol, StreamTokenTracker,
};
pub use policy::{
    adapt_unsupported_reasoning_effort, best_auto_choice, best_auto_choice_matching,
    best_auto_model, best_auto_model_matching, channel_serving_traffic, channel_supports_model,
    parse_upstream_error_message, truncate_echoed, upstream_error_payload, upstream_stream_error,
    upstream_stream_has_output, AutoModelChoice, TraceStep, AUTO_MODEL_NAME,
};
