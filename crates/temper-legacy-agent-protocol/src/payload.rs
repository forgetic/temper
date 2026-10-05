//! Agent entrance and outcome payloads (channel.md, 6–7). Repository roots
//! are supplied by the owner; no unavailable skein file API is invented.
use crate::{Error, Limits, grants, render::Text};
use alloc::boxed::Box;
use skein_lib::{List, bytes};
use temper_channel::{Sizes, payload::v1 as wire, wire::EndpointDescriptor};
use temper_legacy_agent_domain::run::{self, charter, outcome};

pub const REPORT: &[u8] = b"report";
pub const APPROVE: &[u8] = b"approve";
pub const REQUEST: &[u8] = b"request-changes";

pub fn endpoints(values: &[EndpointDescriptor], limits: &Limits) -> Result<(), Error> {
    if values.len() > usize::try_from(limits.endpoints).expect("u32 fits usize") {
        return Err(Error::TooLarge);
    }
    for (index, value) in values.iter().enumerate() {
        if value.host.is_empty()
            || !grants::header(&value.host)
            || value.port == 0
            || value.path.first() != Some(&b'/')
            || !grants::header(&value.path)
            || value.host.len() > usize::try_from(limits.name_bytes).expect("u32 fits usize")
            || value.path.len() > usize::try_from(limits.name_bytes).expect("u32 fits usize")
            || value.effort.len() > usize::try_from(limits.name_bytes).expect("u32 fits usize")
            || !grants::header(&value.effort)
        {
            return Err(Error::Endpoint);
        }
        for other in values.get(index.saturating_add(1)..).unwrap_or_default() {
            if other.endpoint == value.endpoint {
                return Err(Error::Endpoint);
            }
        }
    }
    Ok(())
}
pub fn charter(
    bytes: &[u8],
    checkout: charter::Checkout,
    endpoints: &[EndpointDescriptor],
    sizes: &Sizes,
    limits: &Limits,
) -> Result<run::Charter, Error> {
    self::endpoints(endpoints, limits)?;
    let wire = wire::decode_charter(bytes, sizes).ok_or(Error::Malformed)?;
    let brief = brief(&wire, limits.request_bytes)?;
    let tools = charter::Tools { inspect: true, modify: wire.grants.modify, shell: wire.grants.shell };
    let outlets: Box<[charter::Outlet]> =
        if wire.grants.note { Box::new([charter::Outlet { name: bytes::copy_of(b"note") }]) } else { Box::new([]) };
    let grants = charter::Grants { tools, forge: wire.grants.forge, agents: wire.grants.subagents, outlets };
    let spec = match wire.finish {
        wire::FinishSpec::Report { grows: _ } => {
            outcome::OutcomeSpec { change: None, verdicts: Box::new([rule(REPORT, 0, 0)]) }
        }
        wire::FinishSpec::Change { checks } => {
            outcome::OutcomeSpec { change: Some(outcome::ChangeSpec { checks }), verdicts: Box::new([]) }
        }
        wire::FinishSpec::Verdict => outcome::OutcomeSpec {
            change: None,
            verdicts: Box::new([
                rule(APPROVE, 0, 0),
                outcome::VerdictRule {
                    name: bytes::copy_of(REQUEST),
                    children: outcome::Children { min: 1, max: 8 },
                    kinds: Box::new([bytes::copy_of(b"blocking"), bytes::copy_of(b"nit")]),
                    fields: Box::new([bytes::copy_of(b"path"), bytes::copy_of(b"body")]),
                },
            ]),
        },
        wire::FinishSpec::Turn { .. } => return Err(Error::Unsupported),
    };
    if wire.models.is_empty() {
        return Err(Error::Endpoint);
    }
    let count = u32::try_from(wire.models.len()).or(Err(Error::TooLarge))?;
    let mut models = List::with_capacity(count);
    for model in wire.models {
        if model.model.is_empty() || model.model.contains(&0) || model.max_tokens == 0 {
            return Err(Error::Endpoint);
        }
        for other in &models {
            let charter::Llm { model: other, .. } = other;
            if other == &model.model {
                return Err(Error::Endpoint);
            }
        }
        let mut account = None;
        for endpoint in endpoints {
            if endpoint.endpoint == model.endpoint {
                account = Some(endpoint.account);
            }
        }
        models
            .push(charter::Llm {
                account: account.ok_or(Error::Endpoint)?,
                endpoint: charter::Endpoint(model.endpoint),
                model: model.model,
                max_tokens: model.max_tokens,
            })
            .expect("the source model count");
    }
    let mut all = models.into_boxed().into_iter();
    let llm = all.next().ok_or(Error::Endpoint)?;
    let mut others = List::with_capacity(count.saturating_sub(1));
    for model in all {
        others.push(model).expect("all but the main model");
    }
    let budget = run::Budget::from_tokens(wire.budget.turns, wire.budget.tokens, wire.budget.time);
    Ok(run::Charter { brief, checkout, grants, outcome: spec, budget, llm, models: others.into_boxed() })
}
fn rule(name: &[u8], min: u32, max: u32) -> outcome::VerdictRule {
    outcome::VerdictRule {
        name: bytes::copy_of(name),
        children: outcome::Children { min, max },
        kinds: Box::new([]),
        fields: Box::new([]),
    }
}
/// Only the charter's used LLM accounts, deduplicated; all descriptors remain
/// available to resolve endpoints, and unrelated accounts never gain values.
pub fn scope(charter: &run::Charter, limits: &Limits) -> Result<Box<[u32]>, Error> {
    let mut accounts = List::with_capacity(limits.accounts);
    accounts.push(charter.llm.account).or(Err(Error::TooLarge))?;
    for model in &charter.models {
        let mut known = false;
        for account in &accounts {
            if *account == model.account {
                known = true;
            }
        }
        if !known {
            accounts.push(model.account).or(Err(Error::TooLarge))?;
        }
    }
    Ok(accounts.into_boxed())
}
fn brief(value: &wire::Charter, max: u32) -> Result<Box<[u8]>, Error> {
    let mut measure = Text::measure();
    brief_text(value, &mut measure);
    let mut write = Text::write(measure.length(max)?);
    brief_text(value, &mut write);
    Ok(write.finish())
}
fn brief_text(value: &wire::Charter, out: &mut Text) {
    out.bytes(&value.instructions);
    out.put(b"\n\nWhy: ");
    out.put(match value.why {
        wire::Why::Work => b"work",
        wire::Why::Produce => b"produce",
        wire::Why::Review { .. } => b"review",
        wire::Why::Turn => b"turn",
        wire::Why::Repair { repair: wire::Repair::CiFailed } => b"repair failed CI",
        wire::Why::Repair { repair: wire::Repair::ChangesRequested } => b"repair requested changes",
        wire::Why::Repair { repair: wire::Repair::BaseMoved } => b"repair moved base",
        wire::Why::Repair { repair: wire::Repair::Conflicts } => b"repair conflicts",
    });
    match value.why {
        wire::Why::Review { head } => {
            out.put(b" ");
            for byte in head {
                let digits = b"0123456789abcdef";
                out.put(&[
                    *digits.get(usize::from(byte >> 4_u8)).expect("high nibble"),
                    *digits.get(usize::from(byte & 15)).expect("low nibble"),
                ]);
            }
        }
        wire::Why::Work | wire::Why::Produce | wire::Why::Repair { .. } | wire::Why::Turn => {}
    }
    out.put(b"\n");
    for section in &value.brief {
        out.put(b"\n## ");
        out.put(match section.kind {
            wire::SectionKind::Item => b"Item",
            wire::SectionKind::Comments => b"Comments",
            wire::SectionKind::Dependencies => b"Dependencies",
            wire::SectionKind::Ci => b"CI",
            wire::SectionKind::Reviews => b"Reviews",
            wire::SectionKind::Pull => b"Pull",
            wire::SectionKind::Attempts => b"Attempts",
            wire::SectionKind::Plan => b"Plan",
            wire::SectionKind::Notes => b"Notes",
            wire::SectionKind::Template => b"Template",
        });
        out.put(b"\n");
        match &section.body {
            wire::SectionBody::Text { text } => out.bytes(text),
            wire::SectionBody::Missing { unread } => out.put(match unread {
                wire::Unread::Failed => b"[unread: failed]",
                wire::Unread::Late => b"[unread: late]",
                wire::Unread::Oversized => b"[unread: oversized]",
            }),
        }
        out.put(b"\n");
    }
}
pub fn message(change: &outcome::Change, max: u32) -> Result<Box<[u8]>, Error> {
    let mut measure = Text::measure();
    change_text(change, &mut measure);
    let mut write = Text::write(measure.length(max)?);
    change_text(change, &mut write);
    Ok(write.finish())
}
fn change_text(change: &outcome::Change, out: &mut Text) {
    out.bytes(&change.title);
    if !change.body.is_empty() {
        out.put(b"\n\n");
        out.bytes(&change.body);
    }
}
pub fn outcome(declared: &outcome::Declared, sizes: &Sizes) -> Result<Box<[u8]>, Error> {
    let value = match declared {
        outcome::Declared::Change(change) => wire::Outcome::Change { message: message(change, sizes.outcome)? },
        outcome::Declared::Verdict(verdict) => {
            let mut measure = Text::measure();
            verdict_text(verdict, &mut measure);
            let mut write = Text::write(measure.length(sizes.outcome)?);
            verdict_text(verdict, &mut write);
            let text = write.finish();
            match verdict.name.as_ref() {
                REPORT => wire::Outcome::Report { text },
                APPROVE => wire::Outcome::Verdict { verdict: wire::Verdict::Approve, text },
                REQUEST => wire::Outcome::Verdict { verdict: wire::Verdict::Changes, text },
                _ => return Err(Error::Malformed),
            }
        }
    };
    wire::encode_outcome(&value, sizes).ok_or(Error::TooLarge)
}
fn verdict_text(verdict: &outcome::Verdict, out: &mut Text) {
    out.bytes(&verdict.body);
    for child in &verdict.children {
        out.put(b"\n- ");
        out.bytes(&child.kind);
        for field in &child.fields {
            out.put(b" ");
            out.bytes(&field.name);
            out.put(b": ");
            out.bytes(&field.value);
        }
    }
}
