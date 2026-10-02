//! The good transport: Agent Client Protocol over a stdio subprocess.
//!
//! Tool calls, plans and permission requests arrive as *data*. The alternative
//! is regex over ANSI escapes, which is why this is the path ket prefers and why
//! all three shipped agents are configured to use it — see
//! [`crate::config`] for the handshakes that verified that.
//!
//! Everything here ends up as the same [`crate::event::Event`] values the PTY
//! transport produces. ACP simply observes more: it can say that the agent is
//! running a tool rather than waiting on a model, which is the distinction the
//! whole state machine is built around.

use std::sync::Arc;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    ContentBlock, ErrorCode, InitializeRequest, NewSessionRequest, PermissionOptionKind,
    PromptRequest, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionNotification, SessionUpdate, StopReason,
};
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Agent, ConnectionTo};

use super::{SessionContext, SessionResult};
use crate::KetError;
use crate::config::AgentSpec;
use crate::event::{AgentState, Money, SessionOutcome, SessionUsage};

/// Runs one prompt through an ACP agent.
pub async fn run(spec: &AgentSpec, ctx: &Arc<SessionContext>, prompt: &str) -> SessionResult {
    let mut config = AcpAgentConfig::new(&spec.command).args(spec.args.clone());

    // `AcpAgentConfig` can only *set* variables, so stripping is done by setting
    // them empty. That is enough for the guards this exists to defeat, which are
    // all truthiness checks — an empty string is falsy in every one of them —
    // but it is genuinely weaker than unsetting, and a guard that tested for
    // presence rather than value would survive it. The PTY transport removes
    // them properly.
    for key in &spec.env_remove {
        config = config.env(key, "");
    }
    for (key, value) in &spec.env {
        config = config.env(key, value);
    }

    let agent = AcpAgent::new(config);

    let on_update = Arc::clone(ctx);
    let on_permission = Arc::clone(ctx);
    let driving = Arc::clone(ctx);
    let prompt = prompt.to_owned();
    let agent_name = spec.name.clone();

    let outcome = agent_client_protocol::Client
        .builder()
        .on_receive_notification(
            move |notification: SessionNotification, _cx| {
                let ctx = Arc::clone(&on_update);
                async move {
                    apply(&ctx, notification.update);
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            move |request: RequestPermissionRequest,
                  responder: agent_client_protocol::Responder<RequestPermissionResponse>,
                  _connection| {
                let ctx = Arc::clone(&on_permission);
                async move {
                    let allowed = ctx
                        .request_permission(
                            request.tool_call.tool_call_id.0.as_ref(),
                            tool_kind(&request),
                            &tool_title(&request),
                        )
                        .await;

                    match choose(&request, allowed) {
                        Some(option) => responder.respond(RequestPermissionResponse::new(
                            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                                option,
                            )),
                        )),
                        // The agent offered nothing that matches the decision.
                        // Cancelling the request is the only honest answer:
                        // picking an option that means the opposite would be
                        // worse than refusing.
                        None => responder.respond(RequestPermissionResponse::new(
                            RequestPermissionOutcome::Cancelled,
                        )),
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(agent, move |connection: ConnectionTo<Agent>| async move {
            drive(&driving, &connection, &prompt, &agent_name).await
        })
        .await
        .map_err(|e| KetError::Agent {
            agent: spec.name.clone(),
            why: e.to_string(),
        })?;

    Ok(outcome)
}

/// Handshake, session, prompt.
async fn drive(
    ctx: &Arc<SessionContext>,
    connection: &ConnectionTo<Agent>,
    prompt: &str,
    agent_name: &str,
) -> Result<SessionOutcome, agent_client_protocol::Error> {
    let initialized = connection
        .send_request(InitializeRequest::new(ProtocolVersion::V1))
        .block_task()
        .await?;

    tracing::debug!(
        session = %ctx.id,
        agent = agent_name,
        info = ?initialized.agent_info,
        "acp handshake complete"
    );

    // `session/new` is where authentication actually shows up: an agent that
    // needs a login answers this with `auth_required` rather than failing at
    // `initialize`. Assuming an agent is ready to prompt the moment it spawns is
    // exactly the mistake that leaves a session hanging with no explanation.
    let session = match connection
        .send_request(NewSessionRequest::new(ctx.worktree.clone()))
        .block_task()
        .await
    {
        Ok(session) => session,
        Err(e) if e.code == ErrorCode::AuthRequired => {
            ctx.transition(AgentState::Authenticating);

            // ket cannot complete these flows itself — every one of the three
            // agents delegates to its own CLI login — so the useful thing is to
            // say so, using the agent's own instructions from the handshake.
            let how = initialized
                .auth_methods
                .iter()
                .map(|method| {
                    format!(
                        "{} — {}",
                        method.name(),
                        method.description().unwrap_or("no instructions given")
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");

            return Ok(SessionOutcome::Failed {
                why: format!("{agent_name} needs authentication first: {how}"),
            });
        }
        Err(e) => return Err(e),
    };

    ctx.transition(AgentState::Idle);
    ctx.transition(AgentState::Thinking);

    let mut cancel = ctx.cancel_signal();
    let prompting = connection
        .send_request(PromptRequest::new(
            session.session_id.clone(),
            vec![ContentBlock::from(prompt.to_owned())],
        ))
        .block_task();

    let response = tokio::select! {
        response = prompting => response?,
        () = cancel.cancelled() => {
            // Tell the agent, rather than just dropping the connection: an
            // agent told to stop can tidy up, and one whose socket vanished
            // cannot.
            let _ = connection.send_notification(
                agent_client_protocol::schema::v1::CancelNotification::new(
                    session.session_id.clone(),
                ),
            );
            ctx.transition(AgentState::Idle);
            return Ok(SessionOutcome::Cancelled);
        }
    };

    ctx.transition(AgentState::Idle);

    Ok(match response.stop_reason {
        StopReason::EndTurn => SessionOutcome::Completed,
        StopReason::Cancelled => SessionOutcome::Cancelled,
        // Everything else ended the turn without finishing the work. Reporting
        // these as success would make a truncated run look like a done one.
        other => SessionOutcome::Failed {
            why: format!("{agent_name} stopped: {other:?}"),
        },
    })
}

/// Turns one session update into ket's own events and state.
fn apply(ctx: &SessionContext, update: SessionUpdate) {
    match update {
        // The agent's answer and its reasoning both reach the same stream.
        // Keeping them apart is an Epic 9 concern; losing the reasoning is not
        // an option, since it is most of what you read when reviewing a run.
        SessionUpdate::AgentMessageChunk(chunk) | SessionUpdate::AgentThoughtChunk(chunk) => {
            ctx.transition(AgentState::Thinking);
            if let Some(text) = text_of(&chunk.content) {
                ctx.output(text);
            }
        }

        // This is the distinction PTY cannot make and the reason the state
        // machine has two busy states.
        SessionUpdate::ToolCall(call) => {
            ctx.transition(AgentState::ExecutingTool);
            ctx.output(format!("• {}\n", call.title));
        }

        SessionUpdate::ToolCallUpdate(_) => {}

        // How full the model's context is. Not a state change, but the thing
        // that quietly ruins a long run, so it is worth surfacing rather than
        // discarding — see `SessionUsage`.
        SessionUpdate::UsageUpdate(update) => {
            ctx.record_usage(SessionUsage {
                used: update.used,
                size: update.size,
                cost: update
                    .cost
                    .as_ref()
                    .map(|cost| Money::from_amount(cost.amount, cost.currency.clone())),
            });
        }

        // Plans, mode changes, available commands. Real information, but nothing
        // that changes the session's state and nothing the shell draws yet.
        _ => {}
    }
}

/// Extracts displayable text from a content block.
fn text_of(content: &ContentBlock) -> Option<String> {
    match content {
        ContentBlock::Text(text) => Some(text.text.clone()),
        // Images and embedded resources have no text form worth inventing.
        _ => None,
    }
}

/// The ACP tool kind, as a policy key.
///
/// Deliberately the *kind* rather than the tool's name: `read`, `edit` and
/// `execute` mean the same thing whichever agent is running, so one policy
/// covers the whole rotation. Tool names do not — they are per-agent, and a
/// policy written against them would silently not apply to the other two.
fn tool_kind(request: &RequestPermissionRequest) -> &'static str {
    use agent_client_protocol::schema::v1::ToolKind;

    match request.tool_call.fields.kind {
        Some(ToolKind::Read) => "read",
        Some(ToolKind::Edit) => "edit",
        Some(ToolKind::Delete) => "delete",
        Some(ToolKind::Move) => "move",
        Some(ToolKind::Search) => "search",
        Some(ToolKind::Execute) => "execute",
        Some(ToolKind::Think) => "think",
        Some(ToolKind::Fetch) => "fetch",
        _ => "other",
    }
}

/// The agent's own description of what it wants to do.
fn tool_title(request: &RequestPermissionRequest) -> String {
    request
        .tool_call
        .fields
        .title
        .clone()
        .unwrap_or_else(|| "(no description given)".to_owned())
}

/// Picks the option matching a decision.
///
/// Prefers the "once" forms over the "always" forms in both directions. An
/// "always" answer is a persistent policy change, and ket's policy lives in its
/// own config where it can be read and edited — not hidden inside an agent's
/// memory of a decision someone made in a hurry.
fn choose(
    request: &RequestPermissionRequest,
    allowed: bool,
) -> Option<agent_client_protocol::schema::v1::PermissionOptionId> {
    let (preferred, fallback) = if allowed {
        (
            PermissionOptionKind::AllowOnce,
            PermissionOptionKind::AllowAlways,
        )
    } else {
        (
            PermissionOptionKind::RejectOnce,
            PermissionOptionKind::RejectAlways,
        )
    };

    request
        .options
        .iter()
        .find(|option| option.kind == preferred)
        .or_else(|| {
            request
                .options
                .iter()
                .find(|option| option.kind == fallback)
        })
        .map(|option| option.option_id.clone())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use agent_client_protocol::schema::v1::{
        ContentChunk, PermissionOption, ResourceLink, ToolCall, ToolCallUpdate,
        ToolCallUpdateFields, ToolKind, UsageUpdate,
    };

    use super::*;
    use crate::agent::SessionContext;
    use crate::config::{AgentTimeouts, PermissionConfig};
    use crate::event::{Event, EventBus};
    use crate::id::SessionId;

    fn context() -> (
        Arc<SessionContext>,
        tokio::sync::broadcast::Receiver<crate::event::Envelope>,
    ) {
        let bus = Arc::new(EventBus::default());
        let events = bus.subscribe();
        let (_handle, cancel) = super::super::cancellation();
        let ctx = Arc::new(SessionContext::new(
            SessionId::new("s1"),
            "/tmp",
            bus,
            AgentTimeouts::default(),
            PermissionConfig::default(),
            cancel,
        ));
        (ctx, events)
    }

    fn drain(events: &mut tokio::sync::broadcast::Receiver<crate::event::Envelope>) -> Vec<Event> {
        let mut out = Vec::new();
        while let Ok(envelope) = events.try_recv() {
            out.push(envelope.event);
        }
        out
    }

    fn tool_call_request(kind: Option<ToolKind>, title: Option<&str>) -> RequestPermissionRequest {
        let mut fields = ToolCallUpdateFields::new();
        fields = fields.kind(kind);
        fields = fields.title(title.map(str::to_owned));
        RequestPermissionRequest::new(
            "session-1",
            ToolCallUpdate::new("call-1", fields),
            Vec::new(),
        )
    }

    #[test]
    fn tool_kind_maps_every_acp_kind_to_its_policy_key() {
        let cases = [
            (Some(ToolKind::Read), "read"),
            (Some(ToolKind::Edit), "edit"),
            (Some(ToolKind::Delete), "delete"),
            (Some(ToolKind::Move), "move"),
            (Some(ToolKind::Search), "search"),
            (Some(ToolKind::Execute), "execute"),
            (Some(ToolKind::Think), "think"),
            (Some(ToolKind::Fetch), "fetch"),
            (None, "other"),
        ];
        for (kind, expected) in cases {
            let request = tool_call_request(kind, None);
            assert_eq!(tool_kind(&request), expected, "{kind:?}");
        }
    }

    #[test]
    fn tool_title_falls_back_when_the_agent_gave_none() {
        let with_title = tool_call_request(None, Some("reading the config"));
        assert_eq!(tool_title(&with_title), "reading the config");

        let without_title = tool_call_request(None, None);
        assert_eq!(tool_title(&without_title), "(no description given)");
    }

    #[test]
    fn text_of_extracts_plain_text_and_nothing_else() {
        let text = ContentBlock::from("hello".to_owned());
        assert_eq!(text_of(&text).as_deref(), Some("hello"));

        let link = ContentBlock::ResourceLink(ResourceLink::new("name", "file:///x"));
        assert_eq!(text_of(&link), None);
    }

    #[test]
    fn choose_prefers_the_once_option_over_the_always_option() {
        let request = RequestPermissionRequest::new(
            "s1",
            ToolCallUpdate::new("c1", ToolCallUpdateFields::new()),
            vec![
                PermissionOption::new(
                    "allow-always",
                    "Always Allow",
                    PermissionOptionKind::AllowAlways,
                ),
                PermissionOption::new("allow-once", "Allow", PermissionOptionKind::AllowOnce),
                PermissionOption::new("reject-once", "Reject", PermissionOptionKind::RejectOnce),
                PermissionOption::new(
                    "reject-always",
                    "Always Reject",
                    PermissionOptionKind::RejectAlways,
                ),
            ],
        );

        assert_eq!(
            choose(&request, true).map(|id| id.to_string()),
            Some("allow-once".to_owned())
        );
        assert_eq!(
            choose(&request, false).map(|id| id.to_string()),
            Some("reject-once".to_owned())
        );
    }

    #[test]
    fn choose_falls_back_to_the_always_option_when_once_is_absent() {
        let request = RequestPermissionRequest::new(
            "s1",
            ToolCallUpdate::new("c1", ToolCallUpdateFields::new()),
            vec![PermissionOption::new(
                "allow-always",
                "Always Allow",
                PermissionOptionKind::AllowAlways,
            )],
        );

        assert_eq!(
            choose(&request, true).map(|id| id.to_string()),
            Some("allow-always".to_owned())
        );
        // Asking to reject finds nothing matching among allow-only options.
        assert_eq!(choose(&request, false), None);
    }

    #[test]
    fn choose_is_none_when_nothing_matches_the_decision() {
        let request = RequestPermissionRequest::new(
            "s1",
            ToolCallUpdate::new("c1", ToolCallUpdateFields::new()),
            Vec::new(),
        );
        assert_eq!(choose(&request, true), None);
        assert_eq!(choose(&request, false), None);
    }

    #[test]
    fn apply_a_message_chunk_transitions_to_thinking_and_publishes_the_text() {
        let (ctx, mut events) = context();
        ctx.transition(AgentState::Idle);
        drain(&mut events);
        apply(
            &ctx,
            SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from(
                "hello there".to_owned(),
            ))),
        );

        assert_eq!(ctx.state(), AgentState::Thinking);
        let output: String = drain(&mut events)
            .into_iter()
            .filter_map(|e| match e {
                Event::AgentOutput { chunk, .. } => Some(chunk),
                _ => None,
            })
            .collect();
        assert!(output.contains("hello there"), "{output:?}");
    }

    #[test]
    fn apply_a_tool_call_transitions_to_executing_and_names_it() {
        let (ctx, mut events) = context();
        ctx.transition(AgentState::Idle);
        ctx.transition(AgentState::Thinking);
        drain(&mut events);
        apply(
            &ctx,
            SessionUpdate::ToolCall(ToolCall::new("call-1", "reading a file")),
        );

        assert_eq!(ctx.state(), AgentState::ExecutingTool);
        let output: String = drain(&mut events)
            .into_iter()
            .filter_map(|e| match e {
                Event::AgentOutput { chunk, .. } => Some(chunk),
                _ => None,
            })
            .collect();
        assert!(output.contains("reading a file"), "{output:?}");
    }

    #[test]
    fn apply_a_usage_update_records_usage_without_changing_state() {
        let (ctx, _events) = context();
        let before = ctx.state();
        apply(&ctx, SessionUpdate::UsageUpdate(UsageUpdate::new(10, 100)));
        assert_eq!(ctx.state(), before, "a usage update is not a state change");

        let usage = ctx.usage().expect("recorded");
        assert_eq!(usage.used, 10);
        assert_eq!(usage.size, 100);
        assert!(usage.cost.is_none());
    }

    #[test]
    fn apply_a_usage_update_with_a_cost_converts_it_to_micros() {
        let (ctx, _events) = context();
        apply(
            &ctx,
            SessionUpdate::UsageUpdate(
                UsageUpdate::new(10, 100)
                    .cost(agent_client_protocol::schema::v1::Cost::new(1.5, "USD")),
            ),
        );

        let cost = ctx.usage().expect("recorded").cost.expect("cost set");
        assert_eq!(cost.micros, 1_500_000);
        assert_eq!(cost.currency, "USD");
    }

    #[test]
    fn apply_a_thought_chunk_transitions_to_thinking_and_publishes_the_text() {
        let (ctx, mut events) = context();
        ctx.transition(AgentState::Idle);
        drain(&mut events);
        apply(
            &ctx,
            SessionUpdate::AgentThoughtChunk(ContentChunk::new(ContentBlock::from(
                "considering the options".to_owned(),
            ))),
        );

        assert_eq!(ctx.state(), AgentState::Thinking);
        let output: String = drain(&mut events)
            .into_iter()
            .filter_map(|e| match e {
                Event::AgentOutput { chunk, .. } => Some(chunk),
                _ => None,
            })
            .collect();
        assert!(output.contains("considering the options"), "{output:?}");
    }

    #[test]
    fn apply_a_tool_call_update_does_nothing() {
        let (ctx, mut events) = context();
        let before = ctx.state();
        apply(
            &ctx,
            SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                "call-1",
                ToolCallUpdateFields::new(),
            )),
        );
        assert_eq!(ctx.state(), before);
        assert!(drain(&mut events).is_empty());
    }

    #[test]
    fn apply_an_unlisted_update_variant_falls_through_the_wildcard() {
        let (ctx, mut events) = context();
        let before = ctx.state();
        apply(
            &ctx,
            SessionUpdate::UserMessageChunk(ContentChunk::new(ContentBlock::from(
                "what the user typed".to_owned(),
            ))),
        );
        assert_eq!(ctx.state(), before, "not a state ket tracks");
        assert!(drain(&mut events).is_empty());
    }
}
