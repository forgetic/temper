//! Version 2 wire records; version 1 layouts remain in wire.rs.
use super::{
    AssignmentRefusal, EndpointDescriptor, Failure, Grant, Hosting, RunFailure, Work, get_assignment_refusal,
    get_endpoint_descriptor, get_failure, get_grant, get_hosting, get_run_failure, get_work, put_assignment_refusal,
    put_endpoint_descriptor, put_failure, put_grant, put_hosting, put_run_failure, put_work,
};
use crate::{
    Sizes,
    primitives::{self as p, Encoder},
};
use alloc::boxed::Box;
use core::mem::size_of;
use skein_lib::{Duration, List, Reader};

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Hello {
    pub slots: u32,
    pub workstreams: Box<[Box<[u8]>]>,
    pub hosting: Box<[Hosting]>,
    pub graces: Duration,
    pub push_deadline: Duration,
}
pub(crate) fn put_hello(out: &mut Encoder, value: &Hello, sizes: &Sizes) -> Option<()> {
    let slots = &value.slots;
    out.u32(*slots)?;
    let workstreams = &value.workstreams;
    let count_1 = u32::try_from(workstreams.len()).ok()?;
    if count_1 > sizes.workstreams {
        return None;
    }
    out.u32(count_1)?;
    for element in workstreams {
        out.bytes(element, sizes.name_bytes)?;
    }
    let hosting = &value.hosting;
    let count_2 = u32::try_from(hosting.len()).ok()?;
    if count_2 > sizes.slots {
        return None;
    }
    out.u32(count_2)?;
    for element in hosting {
        put_hosting(out, element, sizes)?;
    }
    let graces = &value.graces;
    out.u64(graces.as_nanos())?;
    let push_deadline = &value.push_deadline;
    out.u64(push_deadline.as_nanos())?;
    Some(())
}
pub(crate) fn get_hello(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Hello> {
    Some(Hello {
        slots: input.u32()?,
        workstreams: {
            let count = p::count(input, sizes.workstreams, 4)?;
            let mut values = List::with_capacity(count);
            for _ in 0..count {
                let value = p::bytes(input, sizes.name_bytes)?;
                values.push(value).expect("validated array capacity");
            }
            values.into_boxed()
        },
        hosting: {
            let count = p::count(input, sizes.slots, 17)?;
            let mut values = List::with_capacity(count);
            for _ in 0..count {
                let value = get_hosting(input, sizes)?;
                values.push(value).expect("validated array capacity");
            }
            values.into_boxed()
        },
        graces: Duration::from_nanos(input.u64()?),
        push_deadline: Duration::from_nanos(input.u64()?),
    })
}
pub(crate) fn heap_hello(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = u64::try_from(size_of::<Box<[u8]>>())
            .ok()?
            .checked_add(Some(u64::from(sizes.name_bytes))?)?
            .checked_mul(u64::from(sizes.workstreams))?;
        total = total.checked_add(field)?;
        let field =
            u64::try_from(size_of::<Hosting>()).ok()?.checked_add(Some(0_u64)?)?.checked_mul(u64::from(sizes.slots))?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}
pub(crate) fn largest_hello(sizes: &Sizes) -> Option<u32> {
    let mut largest = 0_u32;
    let mut length = 0_u32;
    length = length.checked_add(4_u32)?;
    length =
        length.checked_add(4_u32.checked_add(sizes.workstreams.checked_mul(4_u32.checked_add(sizes.name_bytes)?)?)?)?;
    length = length.checked_add(4_u32.checked_add(sizes.slots.checked_mul(17_u32)?)?)?;
    length = length.checked_add(8_u32)?;
    length = length.checked_add(8_u32)?;
    largest = largest.max(length);
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Repository {
    pub tag: u32,
    pub name: Box<[u8]>,
    pub remote: Box<[u8]>,
    pub start: Start,
    pub access: Access,
    pub identity: u32,
}
pub(crate) fn put_repository(out: &mut Encoder, value: &Repository, sizes: &Sizes) -> Option<()> {
    let tag = &value.tag;
    out.u32(*tag)?;
    let name = &value.name;
    out.bytes(name, sizes.name_bytes)?;
    let remote = &value.remote;
    out.bytes(remote, sizes.name_bytes)?;
    let start = &value.start;
    put_start(out, start, sizes)?;
    let access = &value.access;
    put_access(out, access, sizes)?;
    let identity = &value.identity;
    out.u32(*identity)?;
    Some(())
}
pub(crate) fn get_repository(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Repository> {
    Some(Repository {
        tag: input.u32()?,
        name: p::bytes(input, sizes.name_bytes)?,
        remote: p::bytes(input, sizes.name_bytes)?,
        start: get_start(input, sizes)?,
        access: get_access(input, sizes)?,
        identity: input.u32()?,
    })
}
pub(crate) fn heap_repository(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.name_bytes))?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.name_bytes))?;
        total = total.checked_add(field)?;
        let field = heap_start(sizes)?;
        total = total.checked_add(field)?;
        let field = heap_access(sizes)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}
pub(crate) fn largest_repository(sizes: &Sizes) -> Option<u32> {
    let mut largest = 0_u32;
    let mut length = 0_u32;
    length = length.checked_add(4_u32)?;
    length = length.checked_add(4_u32.checked_add(sizes.name_bytes)?)?;
    length = length.checked_add(4_u32.checked_add(sizes.name_bytes)?)?;
    length = length.checked_add(largest_start(sizes)?)?;
    length = length.checked_add(largest_access(sizes)?)?;
    length = length.checked_add(4_u32)?;
    largest = largest.max(length);
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Workspace {
    pub key: Box<[u8]>,
    pub repositories: Box<[Repository]>,
}
pub(crate) fn put_workspace(out: &mut Encoder, value: &Workspace, sizes: &Sizes) -> Option<()> {
    let key = &value.key;
    out.bytes(key, sizes.name_bytes)?;
    let repositories = &value.repositories;
    let count_3 = u32::try_from(repositories.len()).ok()?;
    if count_3 > sizes.repositories {
        return None;
    }
    out.u32(count_3)?;
    for element in repositories {
        put_repository(out, element, sizes)?;
    }
    Some(())
}
pub(crate) fn get_workspace(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Workspace> {
    Some(Workspace {
        key: p::bytes(input, sizes.name_bytes)?,
        repositories: {
            let count = p::count(input, sizes.repositories, 22)?;
            let mut values = List::with_capacity(count);
            for _ in 0..count {
                let value = get_repository(input, sizes)?;
                values.push(value).expect("validated array capacity");
            }
            values.into_boxed()
        },
    })
}
pub(crate) fn heap_workspace(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(u64::from(sizes.name_bytes))?;
        total = total.checked_add(field)?;
        let field = u64::try_from(size_of::<Repository>())
            .ok()?
            .checked_add(heap_repository(sizes)?)?
            .checked_mul(u64::from(sizes.repositories))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}
pub(crate) fn largest_workspace(sizes: &Sizes) -> Option<u32> {
    let mut largest = 0_u32;
    let mut length = 0_u32;
    length = length.checked_add(4_u32.checked_add(sizes.name_bytes)?)?;
    length = length.checked_add(4_u32.checked_add(sizes.repositories.checked_mul(largest_repository(sizes)?)?)?)?;
    largest = largest.max(length);
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Assign {
    pub run: u64,
    pub attempt: u64,
    pub workspace: Workspace,
    pub save: Option<Box<[u8]>>,
    pub charter: Box<[u8]>,
    pub transcript: Option<Box<[u8]>>,
    pub grants: Box<[Grant]>,
}
pub(crate) fn put_assign(out: &mut Encoder, value: &Assign, sizes: &Sizes) -> Option<()> {
    let run = &value.run;
    out.u64(*run)?;
    let attempt = &value.attempt;
    out.u64(*attempt)?;
    let workspace = &value.workspace;
    put_workspace(out, workspace, sizes)?;
    let save = &value.save;
    match save {
        Some(element) => {
            out.u8(1)?;
            out.bytes(element, sizes.name_bytes)?;
        }
        None => {
            out.u8(0)?;
        }
    }
    let charter = &value.charter;
    out.bytes(charter, sizes.charter)?;
    let transcript = &value.transcript;
    match transcript {
        Some(element) => {
            out.u8(1)?;
            out.bytes(element, sizes.transcript)?;
        }
        None => {
            out.u8(0)?;
        }
    }
    let grants = &value.grants;
    let count_4 = u32::try_from(grants.len()).ok()?;
    if count_4 > sizes.grants {
        return None;
    }
    out.u32(count_4)?;
    for element in grants {
        put_grant(out, element, sizes)?;
    }
    Some(())
}
pub(crate) fn get_assign(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Assign> {
    Some(Assign {
        run: input.u64()?,
        attempt: input.u64()?,
        workspace: get_workspace(input, sizes)?,
        save: match input.u8()? {
            0 => None,
            1 => Some(p::bytes(input, sizes.name_bytes)?),
            _ => return None,
        },
        charter: p::bytes(input, sizes.charter)?,
        transcript: match input.u8()? {
            0 => None,
            1 => Some(p::bytes(input, sizes.transcript)?),
            _ => return None,
        },
        grants: {
            let count = p::count(input, sizes.grants, 28)?;
            let mut values = List::with_capacity(count);
            for _ in 0..count {
                let value = get_grant(input, sizes)?;
                values.push(value).expect("validated array capacity");
            }
            values.into_boxed()
        },
    })
}
pub(crate) fn heap_assign(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = heap_workspace(sizes)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.name_bytes))?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.charter))?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.transcript))?;
        total = total.checked_add(field)?;
        let field = u64::try_from(size_of::<Grant>())
            .ok()?
            .checked_add(u64::from(sizes.token_bytes).checked_mul(2)?)?
            .checked_mul(u64::from(sizes.grants))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}
pub(crate) fn largest_assign(sizes: &Sizes) -> Option<u32> {
    let mut largest = 0_u32;
    let mut length = 0_u32;
    length = length.checked_add(8_u32)?;
    length = length.checked_add(8_u32)?;
    length = length.checked_add(largest_workspace(sizes)?)?;
    length = length.checked_add(1_u32.checked_add(4_u32.checked_add(sizes.name_bytes)?)?)?;
    length = length.checked_add(4_u32.checked_add(sizes.charter)?)?;
    length = length.checked_add(1_u32.checked_add(4_u32.checked_add(sizes.transcript)?)?)?;
    length = length.checked_add(
        4_u32.checked_add(sizes.grants.checked_mul(28_u32.checked_add(sizes.token_bytes.checked_mul(2)?)?)?)?,
    )?;
    largest = largest.max(length);
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Answer {
    pub run: u64,
    pub attempt: u64,
    pub turns: u32,
    pub spent: u64,
    pub answer: LinkAnswer,
}
pub(crate) fn put_answer(out: &mut Encoder, value: &Answer, sizes: &Sizes) -> Option<()> {
    let run = &value.run;
    out.u64(*run)?;
    let attempt = &value.attempt;
    out.u64(*attempt)?;
    let turns = &value.turns;
    out.u32(*turns)?;
    let spent = &value.spent;
    out.u64(*spent)?;
    let answer = &value.answer;
    put_link_answer(out, answer, sizes)?;
    Some(())
}
pub(crate) fn get_answer(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Answer> {
    Some(Answer {
        run: input.u64()?,
        attempt: input.u64()?,
        turns: input.u32()?,
        spent: input.u64()?,
        answer: get_link_answer(input, sizes)?,
    })
}
pub(crate) fn heap_answer(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = heap_link_answer(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}
pub(crate) fn largest_answer(sizes: &Sizes) -> Option<u32> {
    let mut largest = 0_u32;
    let mut length = 0_u32;
    length = length.checked_add(8_u32)?;
    length = length.checked_add(8_u32)?;
    length = length.checked_add(4_u32)?;
    length = length.checked_add(8_u32)?;
    length = length.checked_add(largest_link_answer(sizes)?)?;
    largest = largest.max(length);
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AgentRepository {
    pub name: Box<[u8]>,
    pub writable: bool,
    pub conflicts: Box<[Box<[u8]>]>,
}
pub(crate) fn put_agent_repository(out: &mut Encoder, value: &AgentRepository, sizes: &Sizes) -> Option<()> {
    let name = &value.name;
    out.bytes(name, sizes.name_bytes)?;
    let writable = &value.writable;
    out.bool(*writable)?;
    let conflicts = &value.conflicts;
    let count_5 = u32::try_from(conflicts.len()).ok()?;
    if count_5 > sizes.conflicts {
        return None;
    }
    out.u32(count_5)?;
    for element in conflicts {
        out.bytes(element, sizes.name_bytes)?;
    }
    Some(())
}
pub(crate) fn get_agent_repository(input: &mut Reader<'_>, sizes: &Sizes) -> Option<AgentRepository> {
    Some(AgentRepository {
        name: p::bytes(input, sizes.name_bytes)?,
        writable: p::boolean(input)?,
        conflicts: {
            let count = p::count(input, sizes.conflicts, 4)?;
            let mut values = List::with_capacity(count);
            for _ in 0..count {
                let value = p::bytes(input, sizes.name_bytes)?;
                values.push(value).expect("validated array capacity");
            }
            values.into_boxed()
        },
    })
}
pub(crate) fn heap_agent_repository(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(u64::from(sizes.name_bytes))?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = u64::try_from(size_of::<Box<[u8]>>())
            .ok()?
            .checked_add(Some(u64::from(sizes.name_bytes))?)?
            .checked_mul(u64::from(sizes.conflicts))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}
pub(crate) fn largest_agent_repository(sizes: &Sizes) -> Option<u32> {
    let mut largest = 0_u32;
    let mut length = 0_u32;
    length = length.checked_add(4_u32.checked_add(sizes.name_bytes)?)?;
    length = length.checked_add(1_u32)?;
    length =
        length.checked_add(4_u32.checked_add(sizes.conflicts.checked_mul(4_u32.checked_add(sizes.name_bytes)?)?)?)?;
    largest = largest.max(length);
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AgentStart {
    pub charter: Box<[u8]>,
    pub transcript: Option<Box<[u8]>>,
    pub repositories: Box<[AgentRepository]>,
    pub endpoints: Box<[EndpointDescriptor]>,
    pub grants: Box<[Grant]>,
}
pub(crate) fn put_agent_start(out: &mut Encoder, value: &AgentStart, sizes: &Sizes) -> Option<()> {
    let charter = &value.charter;
    out.bytes(charter, sizes.charter)?;
    let transcript = &value.transcript;
    match transcript {
        Some(element) => {
            out.u8(1)?;
            out.bytes(element, sizes.transcript)?;
        }
        None => {
            out.u8(0)?;
        }
    }
    let repositories = &value.repositories;
    let count_6 = u32::try_from(repositories.len()).ok()?;
    if count_6 > sizes.repositories {
        return None;
    }
    out.u32(count_6)?;
    for element in repositories {
        put_agent_repository(out, element, sizes)?;
    }
    let endpoints = &value.endpoints;
    let count_7 = u32::try_from(endpoints.len()).ok()?;
    if count_7 > sizes.endpoints {
        return None;
    }
    out.u32(count_7)?;
    for element in endpoints {
        put_endpoint_descriptor(out, element, sizes)?;
    }
    let grants = &value.grants;
    let count_8 = u32::try_from(grants.len()).ok()?;
    if count_8 > sizes.grants {
        return None;
    }
    out.u32(count_8)?;
    for element in grants {
        put_grant(out, element, sizes)?;
    }
    Some(())
}
pub(crate) fn get_agent_start(input: &mut Reader<'_>, sizes: &Sizes) -> Option<AgentStart> {
    Some(AgentStart {
        charter: p::bytes(input, sizes.charter)?,
        transcript: match input.u8()? {
            0 => None,
            1 => Some(p::bytes(input, sizes.transcript)?),
            _ => return None,
        },
        repositories: {
            let count = p::count(input, sizes.repositories, 9)?;
            let mut values = List::with_capacity(count);
            for _ in 0..count {
                let value = get_agent_repository(input, sizes)?;
                values.push(value).expect("validated array capacity");
            }
            values.into_boxed()
        },
        endpoints: {
            let count = p::count(input, sizes.endpoints, 21)?;
            let mut values = List::with_capacity(count);
            for _ in 0..count {
                let value = get_endpoint_descriptor(input, sizes)?;
                values.push(value).expect("validated array capacity");
            }
            values.into_boxed()
        },
        grants: {
            let count = p::count(input, sizes.grants, 28)?;
            let mut values = List::with_capacity(count);
            for _ in 0..count {
                let value = get_grant(input, sizes)?;
                values.push(value).expect("validated array capacity");
            }
            values.into_boxed()
        },
    })
}
pub(crate) fn heap_agent_start(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(u64::from(sizes.charter))?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.transcript))?;
        total = total.checked_add(field)?;
        let field = u64::try_from(size_of::<AgentRepository>())
            .ok()?
            .checked_add(heap_agent_repository(sizes)?)?
            .checked_mul(u64::from(sizes.repositories))?;
        total = total.checked_add(field)?;
        let field = u64::try_from(size_of::<EndpointDescriptor>())
            .ok()?
            .checked_add(u64::from(sizes.name_bytes).checked_mul(3)?)?
            .checked_mul(u64::from(sizes.endpoints))?;
        total = total.checked_add(field)?;
        let field = u64::try_from(size_of::<Grant>())
            .ok()?
            .checked_add(u64::from(sizes.token_bytes).checked_mul(2)?)?
            .checked_mul(u64::from(sizes.grants))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}
pub(crate) fn largest_agent_start(sizes: &Sizes) -> Option<u32> {
    let mut largest = 0_u32;
    let mut length = 0_u32;
    length = length.checked_add(4_u32.checked_add(sizes.charter)?)?;
    length = length.checked_add(1_u32.checked_add(4_u32.checked_add(sizes.transcript)?)?)?;
    length =
        length.checked_add(4_u32.checked_add(sizes.repositories.checked_mul(largest_agent_repository(sizes)?)?)?)?;
    length =
        length
            .checked_add(4_u32.checked_add(
                sizes.endpoints.checked_mul(crate::sizes::bounds::largest_endpoint_descriptor(sizes)?)?,
            )?)?;
    length = length.checked_add(
        4_u32.checked_add(sizes.grants.checked_mul(28_u32.checked_add(sizes.token_bytes.checked_mul(2)?)?)?)?,
    )?;
    largest = largest.max(length);
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Finished {
    pub turns: u32,
    pub spent: u64,
    pub finish: Finish,
}
pub(crate) fn put_finished(out: &mut Encoder, value: &Finished, sizes: &Sizes) -> Option<()> {
    let turns = &value.turns;
    out.u32(*turns)?;
    let spent = &value.spent;
    out.u64(*spent)?;
    let finish = &value.finish;
    put_finish(out, finish, sizes)?;
    Some(())
}
pub(crate) fn get_finished(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Finished> {
    Some(Finished { turns: input.u32()?, spent: input.u64()?, finish: get_finish(input, sizes)? })
}
pub(crate) fn heap_finished(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = heap_finish(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}
pub(crate) fn largest_finished(sizes: &Sizes) -> Option<u32> {
    let mut largest = 0_u32;
    let mut length = 0_u32;
    length = length.checked_add(4_u32)?;
    length = length.checked_add(8_u32)?;
    length = length.checked_add(largest_finish(sizes)?)?;
    largest = largest.max(length);
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AgentCall {
    pub call: u64,
    pub ask: Ask,
}
pub(crate) fn put_agent_call(out: &mut Encoder, value: &AgentCall, sizes: &Sizes) -> Option<()> {
    let call = &value.call;
    out.u64(*call)?;
    let ask = &value.ask;
    put_ask(out, ask, sizes)?;
    Some(())
}
pub(crate) fn get_agent_call(input: &mut Reader<'_>, sizes: &Sizes) -> Option<AgentCall> {
    Some(AgentCall { call: input.u64()?, ask: get_ask(input, sizes)? })
}
pub(crate) fn heap_agent_call(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = heap_ask(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}
pub(crate) fn largest_agent_call(sizes: &Sizes) -> Option<u32> {
    let mut largest = 0_u32;
    let mut length = 0_u32;
    length = length.checked_add(8_u32)?;
    length = length.checked_add(largest_ask(sizes)?)?;
    largest = largest.max(length);
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Turn {
    pub run: u64,
    pub attempt: u64,
    pub turn: u32,
    pub spent: u64,
    pub read: Option<u64>,
    pub body: Box<[u8]>,
}
pub(crate) fn put_turn(out: &mut Encoder, value: &Turn, sizes: &Sizes) -> Option<()> {
    let run = &value.run;
    out.u64(*run)?;
    let attempt = &value.attempt;
    out.u64(*attempt)?;
    let turn = &value.turn;
    out.u32(*turn)?;
    let spent = &value.spent;
    out.u64(*spent)?;
    let read = &value.read;
    match read {
        Some(element) => {
            out.u8(1)?;
            out.u64(*element)?;
        }
        None => {
            out.u8(0)?;
        }
    }
    let body = &value.body;
    out.bytes(body, sizes.turn)?;
    Some(())
}
pub(crate) fn get_turn(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Turn> {
    Some(Turn {
        run: input.u64()?,
        attempt: input.u64()?,
        turn: input.u32()?,
        spent: input.u64()?,
        read: match input.u8()? {
            0 => None,
            1 => Some(input.u64()?),
            _ => return None,
        },
        body: p::bytes(input, sizes.turn)?,
    })
}
pub(crate) fn heap_turn(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.turn))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}
pub(crate) fn largest_turn(sizes: &Sizes) -> Option<u32> {
    let mut largest = 0_u32;
    let mut length = 0_u32;
    length = length.checked_add(8_u32)?;
    length = length.checked_add(8_u32)?;
    length = length.checked_add(4_u32)?;
    length = length.checked_add(8_u32)?;
    length = length.checked_add(1_u32.checked_add(8_u32)?)?;
    length = length.checked_add(4_u32.checked_add(sizes.turn)?)?;
    largest = largest.max(length);
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TurnName {
    pub run: u64,
    pub attempt: u64,
    pub turn: u32,
}
pub(crate) fn put_turn_name(out: &mut Encoder, value: &TurnName, _sizes: &Sizes) -> Option<()> {
    let run = &value.run;
    out.u64(*run)?;
    let attempt = &value.attempt;
    out.u64(*attempt)?;
    let turn = &value.turn;
    out.u32(*turn)?;
    Some(())
}
pub(crate) fn get_turn_name(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<TurnName> {
    Some(TurnName { run: input.u64()?, attempt: input.u64()?, turn: input.u32()? })
}
pub(crate) fn heap_turn_name(_sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}
pub(crate) fn largest_turn_name(_sizes: &Sizes) -> Option<u32> {
    let mut largest = 0_u32;
    let mut length = 0_u32;
    length = length.checked_add(8_u32)?;
    length = length.checked_add(8_u32)?;
    length = length.checked_add(4_u32)?;
    largest = largest.max(length);
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AgentTurn {
    pub turn: u32,
    pub spent: u64,
    pub read: Option<u64>,
    pub body: Box<[u8]>,
}
pub(crate) fn put_agent_turn(out: &mut Encoder, value: &AgentTurn, sizes: &Sizes) -> Option<()> {
    let turn = &value.turn;
    out.u32(*turn)?;
    let spent = &value.spent;
    out.u64(*spent)?;
    let read = &value.read;
    match read {
        Some(element) => {
            out.u8(1)?;
            out.u64(*element)?;
        }
        None => {
            out.u8(0)?;
        }
    }
    let body = &value.body;
    out.bytes(body, sizes.turn)?;
    Some(())
}
pub(crate) fn get_agent_turn(input: &mut Reader<'_>, sizes: &Sizes) -> Option<AgentTurn> {
    Some(AgentTurn {
        turn: input.u32()?,
        spent: input.u64()?,
        read: match input.u8()? {
            0 => None,
            1 => Some(input.u64()?),
            _ => return None,
        },
        body: p::bytes(input, sizes.turn)?,
    })
}
pub(crate) fn heap_agent_turn(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.turn))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}
pub(crate) fn largest_agent_turn(sizes: &Sizes) -> Option<u32> {
    let mut largest = 0_u32;
    let mut length = 0_u32;
    length = length.checked_add(4_u32)?;
    length = length.checked_add(8_u32)?;
    length = length.checked_add(1_u32.checked_add(8_u32)?)?;
    length = length.checked_add(4_u32.checked_add(sizes.turn)?)?;
    largest = largest.max(length);
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Start {
    Branch { branch: Box<[u8]> },
    Commit { commit: [u8; 32] },
    Saved { branch: Box<[u8]> },
    Merge { branch: Box<[u8]>, base: Box<[u8]> },
}
pub(crate) fn put_start(out: &mut Encoder, value: &Start, sizes: &Sizes) -> Option<()> {
    match value {
        Start::Branch { branch } => {
            out.u8(0)?;
            out.bytes(branch, sizes.name_bytes)?;
        }
        Start::Commit { commit } => {
            out.u8(1)?;
            out.raw(commit)?;
        }
        Start::Saved { branch } => {
            out.u8(2)?;
            out.bytes(branch, sizes.name_bytes)?;
        }
        Start::Merge { branch, base } => {
            out.u8(3)?;
            out.bytes(branch, sizes.name_bytes)?;
            out.bytes(base, sizes.name_bytes)?;
        }
    }
    Some(())
}
pub(crate) fn get_start(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Start> {
    match input.u8()? {
        0 => Some(Start::Branch { branch: p::bytes(input, sizes.name_bytes)? }),
        1 => Some(Start::Commit { commit: input.bytes(32)?.try_into().ok()? }),
        2 => Some(Start::Saved { branch: p::bytes(input, sizes.name_bytes)? }),
        3 => {
            Some(Start::Merge { branch: p::bytes(input, sizes.name_bytes)?, base: p::bytes(input, sizes.name_bytes)? })
        }
        _ => None,
    }
}
pub(crate) fn heap_start(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(u64::from(sizes.name_bytes))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(u64::from(sizes.name_bytes))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(u64::from(sizes.name_bytes))?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.name_bytes))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}
pub(crate) fn largest_start(sizes: &Sizes) -> Option<u32> {
    let mut largest = 0_u32;
    let mut length = 1_u32;
    length = length.checked_add(4_u32.checked_add(sizes.name_bytes)?)?;
    largest = largest.max(length);
    let mut length = 1_u32;
    length = length.checked_add(32_u32)?;
    largest = largest.max(length);
    let mut length = 1_u32;
    length = length.checked_add(4_u32.checked_add(sizes.name_bytes)?)?;
    largest = largest.max(length);
    let mut length = 1_u32;
    length = length.checked_add(4_u32.checked_add(sizes.name_bytes)?)?;
    length = length.checked_add(4_u32.checked_add(sizes.name_bytes)?)?;
    largest = largest.max(length);
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Access {
    ReadOnly,
    Writable { push: Box<[u8]>, expected: Option<[u8; 32]> },
}
pub(crate) fn put_access(out: &mut Encoder, value: &Access, sizes: &Sizes) -> Option<()> {
    match value {
        Access::ReadOnly => {
            out.u8(0)?;
        }
        Access::Writable { push, expected } => {
            out.u8(1)?;
            out.bytes(push, sizes.name_bytes)?;
            match expected {
                Some(element) => {
                    out.u8(1)?;
                    out.raw(element)?;
                }
                None => {
                    out.u8(0)?;
                }
            }
        }
    }
    Some(())
}
pub(crate) fn get_access(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Access> {
    match input.u8()? {
        0 => Some(Access::ReadOnly),
        1 => Some(Access::Writable {
            push: p::bytes(input, sizes.name_bytes)?,
            expected: match input.u8()? {
                0 => None,
                1 => Some(input.bytes(32)?.try_into().ok()?),
                _ => return None,
            },
        }),
        _ => None,
    }
}
pub(crate) fn heap_access(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(u64::from(sizes.name_bytes))?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}
pub(crate) fn largest_access(sizes: &Sizes) -> Option<u32> {
    let mut largest = 0_u32;
    let length = 1_u32;
    largest = largest.max(length);
    let mut length = 1_u32;
    length = length.checked_add(4_u32.checked_add(sizes.name_bytes)?)?;
    length = length.checked_add(1_u32.checked_add(32_u32)?)?;
    largest = largest.max(length);
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum LinkAnswer {
    Refused { refusal: AssignmentRefusal },
    Ended { outcome: Box<[u8]>, work: Work },
    Parked { work: Work },
    Failed { failure: Failure, detail: Box<[u8]>, work: Work },
}
pub(crate) fn put_link_answer(out: &mut Encoder, value: &LinkAnswer, sizes: &Sizes) -> Option<()> {
    match value {
        LinkAnswer::Refused { refusal } => {
            out.u8(0)?;
            put_assignment_refusal(out, refusal, sizes)?;
        }
        LinkAnswer::Ended { outcome, work } => {
            out.u8(1)?;
            out.bytes(outcome, sizes.outcome)?;
            put_work(out, work, sizes)?;
        }
        LinkAnswer::Parked { work } => {
            out.u8(2)?;
            put_work(out, work, sizes)?;
        }
        LinkAnswer::Failed { failure, detail, work } => {
            out.u8(3)?;
            put_failure(out, failure, sizes)?;
            out.bytes(detail, sizes.detail)?;
            put_work(out, work, sizes)?;
        }
    }
    Some(())
}
pub(crate) fn get_link_answer(input: &mut Reader<'_>, sizes: &Sizes) -> Option<LinkAnswer> {
    match input.u8()? {
        0 => Some(LinkAnswer::Refused { refusal: get_assignment_refusal(input, sizes)? }),
        1 => Some(LinkAnswer::Ended { outcome: p::bytes(input, sizes.outcome)?, work: get_work(input, sizes)? }),
        2 => Some(LinkAnswer::Parked { work: get_work(input, sizes)? }),
        3 => Some(LinkAnswer::Failed {
            failure: get_failure(input, sizes)?,
            detail: p::bytes(input, sizes.detail)?,
            work: get_work(input, sizes)?,
        }),
        _ => None,
    }
}
pub(crate) fn heap_link_answer(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(u64::from(sizes.outcome))?;
        total = total.checked_add(field)?;
        let field = crate::memory::heap_work(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = crate::memory::heap_work(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        let field = crate::memory::heap_work(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}
pub(crate) fn largest_link_answer(sizes: &Sizes) -> Option<u32> {
    let mut largest = 0_u32;
    let mut length = 1_u32;
    length = length.checked_add(crate::sizes::bounds::largest_assignment_refusal(sizes)?)?;
    largest = largest.max(length);
    let mut length = 1_u32;
    length = length.checked_add(4_u32.checked_add(sizes.outcome)?)?;
    length = length.checked_add(crate::sizes::bounds::largest_work(sizes)?)?;
    largest = largest.max(length);
    let mut length = 1_u32;
    length = length.checked_add(crate::sizes::bounds::largest_work(sizes)?)?;
    largest = largest.max(length);
    let mut length = 1_u32;
    length = length.checked_add(crate::sizes::bounds::largest_failure(sizes)?)?;
    length = length.checked_add(4_u32.checked_add(sizes.detail)?)?;
    length = length.checked_add(crate::sizes::bounds::largest_work(sizes)?)?;
    largest = largest.max(length);
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Finish {
    Ended { outcome: Box<[u8]> },
    Parked,
    Failed { failure: RunFailure },
}
pub(crate) fn put_finish(out: &mut Encoder, value: &Finish, sizes: &Sizes) -> Option<()> {
    match value {
        Finish::Ended { outcome } => {
            out.u8(0)?;
            out.bytes(outcome, sizes.outcome)?;
        }
        Finish::Parked => {
            out.u8(1)?;
        }
        Finish::Failed { failure } => {
            out.u8(2)?;
            put_run_failure(out, failure, sizes)?;
        }
    }
    Some(())
}
pub(crate) fn get_finish(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Finish> {
    match input.u8()? {
        0 => Some(Finish::Ended { outcome: p::bytes(input, sizes.outcome)? }),
        1 => Some(Finish::Parked),
        2 => Some(Finish::Failed { failure: get_run_failure(input, sizes)? }),
        _ => None,
    }
}
pub(crate) fn heap_finish(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(u64::from(sizes.outcome))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}
pub(crate) fn largest_finish(sizes: &Sizes) -> Option<u32> {
    let mut largest = 0_u32;
    let mut length = 1_u32;
    length = length.checked_add(4_u32.checked_add(sizes.outcome)?)?;
    largest = largest.max(length);
    let length = 1_u32;
    largest = largest.max(length);
    let mut length = 1_u32;
    length = length.checked_add(1_u32)?;
    largest = largest.max(length);
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Ask {
    Push { title: Box<[u8]>, body: Box<[u8]> },
    Relay { body: Box<[u8]> },
}
pub(crate) fn put_ask(out: &mut Encoder, value: &Ask, sizes: &Sizes) -> Option<()> {
    match value {
        Ask::Push { title, body } => {
            out.u8(0)?;
            out.bytes(title, sizes.detail)?;
            out.bytes(body, sizes.detail)?;
        }
        Ask::Relay { body } => {
            out.u8(1)?;
            out.bytes(body, sizes.call)?;
        }
    }
    Some(())
}
pub(crate) fn get_ask(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Ask> {
    match input.u8()? {
        0 => Some(Ask::Push { title: p::bytes(input, sizes.detail)?, body: p::bytes(input, sizes.detail)? }),
        1 => Some(Ask::Relay { body: p::bytes(input, sizes.call)? }),
        _ => None,
    }
}
pub(crate) fn heap_ask(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(u64::from(sizes.call))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}
pub(crate) fn largest_ask(sizes: &Sizes) -> Option<u32> {
    let mut largest = 0_u32;
    let mut length = 1_u32;
    length = length.checked_add(4_u32.checked_add(sizes.detail)?)?;
    length = length.checked_add(4_u32.checked_add(sizes.detail)?)?;
    largest = largest.max(length);
    let mut length = 1_u32;
    length = length.checked_add(4_u32.checked_add(sizes.call)?)?;
    largest = largest.max(length);
    Some(largest)
}
