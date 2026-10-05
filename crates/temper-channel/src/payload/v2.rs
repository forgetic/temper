//! Version 2 payloads: concrete task, authority and connector values.
//! Text and provider turn blocks are bytes; domain actions are typed.
use crate::{
    Sizes,
    primitives::{self as p, Encoder},
};
use alloc::boxed::Box;
use core::mem::size_of;
use skein_lib::{Duration, List, Reader};

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Budget {
    pub spend: u64,
    pub turns: u32,
    pub time: Duration,
}
pub(crate) fn put_budget(out: &mut Encoder, value: &Budget, _sizes: &Sizes) -> Option<()> {
    let spend = &value.spend;
    out.u64(*spend)?;
    let turns = &value.turns;
    out.u32(*turns)?;
    let time = &value.time;
    out.u64(time.as_nanos())?;
    Some(())
}
pub(crate) fn get_budget(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<Budget> {
    Some(Budget { spend: input.u64()?, turns: input.u32()?, time: Duration::from_nanos(input.u64()?) })
}
#[must_use]
pub fn encode_budget(value: &Budget, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_budget(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_budget(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_budget(bytes: &[u8], sizes: &Sizes) -> Option<Budget> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_budget(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_budget(_sizes: &Sizes) -> Option<u64> {
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

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Prices {
    pub input: u64,
    pub cached: u64,
    pub output: u64,
    pub unit: u32,
}
pub(crate) fn put_prices(out: &mut Encoder, value: &Prices, _sizes: &Sizes) -> Option<()> {
    if value.unit == 0 {
        return None;
    }
    let input = &value.input;
    out.u64(*input)?;
    let cached = &value.cached;
    out.u64(*cached)?;
    let output = &value.output;
    out.u64(*output)?;
    let unit = &value.unit;
    out.u32(*unit)?;
    Some(())
}
pub(crate) fn get_prices(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<Prices> {
    let value = Prices { input: input.u64()?, cached: input.u64()?, output: input.u64()?, unit: input.u32()? };
    if value.unit == 0 { None } else { Some(value) }
}
#[must_use]
pub fn encode_prices(value: &Prices, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_prices(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_prices(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_prices(bytes: &[u8], sizes: &Sizes) -> Option<Prices> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_prices(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_prices(_sizes: &Sizes) -> Option<u64> {
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
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Model {
    pub endpoint: u32,
    pub kind: ModelKind,
    pub model: Box<[u8]>,
    pub max_tokens: u32,
    pub prices: Prices,
}
pub(crate) fn put_model(out: &mut Encoder, value: &Model, sizes: &Sizes) -> Option<()> {
    let endpoint = &value.endpoint;
    out.u32(*endpoint)?;
    let kind = &value.kind;
    put_model_kind(out, kind, sizes)?;
    let model = &value.model;
    out.bytes(model, sizes.name_bytes)?;
    let max_tokens = &value.max_tokens;
    out.u32(*max_tokens)?;
    let prices = &value.prices;
    put_prices(out, prices, sizes)?;
    Some(())
}
pub(crate) fn get_model(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Model> {
    Some(Model {
        endpoint: input.u32()?,
        kind: get_model_kind(input, sizes)?,
        model: p::bytes(input, sizes.name_bytes)?,
        max_tokens: input.u32()?,
        prices: get_prices(input, sizes)?,
    })
}
#[must_use]
pub fn encode_model(value: &Model, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_model(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_model(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_model(bytes: &[u8], sizes: &Sizes) -> Option<Model> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_model(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_model(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = heap_model_kind(sizes)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.name_bytes))?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = heap_prices(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Section {
    pub kind: SectionKind,
    pub words: Box<[u8]>,
}
pub(crate) fn put_section(out: &mut Encoder, value: &Section, sizes: &Sizes) -> Option<()> {
    let kind = &value.kind;
    put_section_kind(out, kind, sizes)?;
    let words = &value.words;
    out.bytes(words, sizes.detail)?;
    Some(())
}
pub(crate) fn get_section(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Section> {
    Some(Section { kind: get_section_kind(input, sizes)?, words: p::bytes(input, sizes.detail)? })
}
#[must_use]
pub fn encode_section(value: &Section, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_section(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_section(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_section(bytes: &[u8], sizes: &Sizes) -> Option<Section> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_section(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_section(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = heap_section_kind(sizes)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FieldRule {
    pub name: Box<[u8]>,
    pub required: bool,
}
pub(crate) fn put_field_rule(out: &mut Encoder, value: &FieldRule, sizes: &Sizes) -> Option<()> {
    let name = &value.name;
    out.bytes(name, sizes.name_bytes)?;
    let required = &value.required;
    out.bool(*required)?;
    Some(())
}
pub(crate) fn get_field_rule(input: &mut Reader<'_>, sizes: &Sizes) -> Option<FieldRule> {
    Some(FieldRule { name: p::bytes(input, sizes.name_bytes)?, required: p::boolean(input)? })
}
#[must_use]
pub fn encode_field_rule(value: &FieldRule, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_field_rule(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_field_rule(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_field_rule(bytes: &[u8], sizes: &Sizes) -> Option<FieldRule> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_field_rule(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_field_rule(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
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

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct VerdictRule {
    pub number: u32,
    pub name: Box<[u8]>,
    pub fields: Box<[FieldRule]>,
    pub follow_up_kinds: Box<[u32]>,
    pub most_follow_ups: u32,
}
pub(crate) fn put_verdict_rule(out: &mut Encoder, value: &VerdictRule, sizes: &Sizes) -> Option<()> {
    let number = &value.number;
    out.u32(*number)?;
    let name = &value.name;
    out.bytes(name, sizes.name_bytes)?;
    let fields = &value.fields;
    let count_1 = u32::try_from(fields.len()).ok()?;
    if count_1 > sizes.entries {
        return None;
    }
    out.u32(count_1)?;
    for element in fields {
        put_field_rule(out, element, sizes)?;
    }
    let follow_up_kinds = &value.follow_up_kinds;
    let count_2 = u32::try_from(follow_up_kinds.len()).ok()?;
    if count_2 > sizes.entries {
        return None;
    }
    out.u32(count_2)?;
    for element in follow_up_kinds {
        out.u32(*element)?;
    }
    let most_follow_ups = &value.most_follow_ups;
    out.u32(*most_follow_ups)?;
    Some(())
}
pub(crate) fn get_verdict_rule(input: &mut Reader<'_>, sizes: &Sizes) -> Option<VerdictRule> {
    Some(VerdictRule {
        number: input.u32()?,
        name: p::bytes(input, sizes.name_bytes)?,
        fields: {
            let count = p::count(input, sizes.entries, 5)?;
            let mut values = List::with_capacity(count);
            for _ in 0..count {
                let value = get_field_rule(input, sizes)?;
                values.push(value).expect("validated array capacity");
            }
            values.into_boxed()
        },
        follow_up_kinds: {
            let count = p::count(input, sizes.entries, 4)?;
            let mut values = List::with_capacity(count);
            for _ in 0..count {
                let value = input.u32()?;
                values.push(value).expect("validated array capacity");
            }
            values.into_boxed()
        },
        most_follow_ups: input.u32()?,
    })
}
#[must_use]
pub fn encode_verdict_rule(value: &VerdictRule, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_verdict_rule(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_verdict_rule(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_verdict_rule(bytes: &[u8], sizes: &Sizes) -> Option<VerdictRule> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_verdict_rule(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_verdict_rule(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.name_bytes))?;
        total = total.checked_add(field)?;
        let field = u64::try_from(size_of::<FieldRule>())
            .ok()?
            .checked_add(heap_field_rule(sizes)?)?
            .checked_mul(u64::from(sizes.entries))?;
        total = total.checked_add(field)?;
        let field =
            u64::try_from(size_of::<u32>()).ok()?.checked_add(Some(0_u64)?)?.checked_mul(u64::from(sizes.entries))?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Charter {
    pub instructions: Box<[u8]>,
    pub brief: Box<[Section]>,
    pub tools: u64,
    pub contract: Contract,
    pub budget: Budget,
    pub models: Box<[Model]>,
    pub waiting: Duration,
}
pub(crate) fn put_charter(out: &mut Encoder, value: &Charter, sizes: &Sizes) -> Option<()> {
    let instructions = &value.instructions;
    out.bytes(instructions, sizes.detail)?;
    let brief = &value.brief;
    let count_3 = u32::try_from(brief.len()).ok()?;
    if count_3 > sizes.entries {
        return None;
    }
    out.u32(count_3)?;
    for element in brief {
        put_section(out, element, sizes)?;
    }
    let tools = &value.tools;
    out.u64(*tools)?;
    let contract = &value.contract;
    put_contract(out, contract, sizes)?;
    let budget = &value.budget;
    put_budget(out, budget, sizes)?;
    let models = &value.models;
    let count_4 = u32::try_from(models.len()).ok()?;
    if count_4 > sizes.entries {
        return None;
    }
    out.u32(count_4)?;
    for element in models {
        put_model(out, element, sizes)?;
    }
    let waiting = &value.waiting;
    out.u64(waiting.as_nanos())?;
    Some(())
}
pub(crate) fn get_charter(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Charter> {
    Some(Charter {
        instructions: p::bytes(input, sizes.detail)?,
        brief: {
            let count = p::count(input, sizes.entries, 5)?;
            let mut values = List::with_capacity(count);
            for _ in 0..count {
                let value = get_section(input, sizes)?;
                values.push(value).expect("validated array capacity");
            }
            values.into_boxed()
        },
        tools: input.u64()?,
        contract: get_contract(input, sizes)?,
        budget: get_budget(input, sizes)?,
        models: {
            let count = p::count(input, sizes.entries, 41)?;
            let mut values = List::with_capacity(count);
            for _ in 0..count {
                let value = get_model(input, sizes)?;
                values.push(value).expect("validated array capacity");
            }
            values.into_boxed()
        },
        waiting: Duration::from_nanos(input.u64()?),
    })
}
#[must_use]
pub fn encode_charter(value: &Charter, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_charter(&mut out, value, sizes)?;
    if out.length() > sizes.charter {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_charter(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_charter(bytes: &[u8], sizes: &Sizes) -> Option<Charter> {
    if u32::try_from(bytes.len()).ok()? > sizes.charter {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_charter(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_charter(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        let field = u64::try_from(size_of::<Section>())
            .ok()?
            .checked_add(heap_section(sizes)?)?
            .checked_mul(u64::from(sizes.entries))?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = heap_contract(sizes)?;
        total = total.checked_add(field)?;
        let field = heap_budget(sizes)?;
        total = total.checked_add(field)?;
        let field = u64::try_from(size_of::<Model>())
            .ok()?
            .checked_add(heap_model(sizes)?)?
            .checked_mul(u64::from(sizes.entries))?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Name {
    pub segments: Box<[Box<[u8]>]>,
}
pub(crate) fn put_name(out: &mut Encoder, value: &Name, sizes: &Sizes) -> Option<()> {
    let segments = &value.segments;
    let count_5 = u32::try_from(segments.len()).ok()?;
    if count_5 > sizes.entries {
        return None;
    }
    out.u32(count_5)?;
    for element in segments {
        out.bytes(element, sizes.name_bytes)?;
    }
    Some(())
}
pub(crate) fn get_name(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Name> {
    Some(Name {
        segments: {
            let count = p::count(input, sizes.entries, 4)?;
            let mut values = List::with_capacity(count);
            for _ in 0..count {
                let value = p::bytes(input, sizes.name_bytes)?;
                values.push(value).expect("validated array capacity");
            }
            values.into_boxed()
        },
    })
}
#[must_use]
pub fn encode_name(value: &Name, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_name(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_name(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_name(bytes: &[u8], sizes: &Sizes) -> Option<Name> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_name(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_name(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = u64::try_from(size_of::<Box<[u8]>>())
            .ok()?
            .checked_add(Some(u64::from(sizes.name_bytes))?)?
            .checked_mul(u64::from(sizes.entries))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Pattern {
    pub segments: Box<[Box<[u8]>]>,
    pub last: Last,
}
pub(crate) fn put_pattern(out: &mut Encoder, value: &Pattern, sizes: &Sizes) -> Option<()> {
    let segments = &value.segments;
    let count_6 = u32::try_from(segments.len()).ok()?;
    if count_6 > sizes.entries {
        return None;
    }
    out.u32(count_6)?;
    for element in segments {
        out.bytes(element, sizes.name_bytes)?;
    }
    let last = &value.last;
    put_last(out, last, sizes)?;
    Some(())
}
pub(crate) fn get_pattern(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Pattern> {
    Some(Pattern {
        segments: {
            let count = p::count(input, sizes.entries, 4)?;
            let mut values = List::with_capacity(count);
            for _ in 0..count {
                let value = p::bytes(input, sizes.name_bytes)?;
                values.push(value).expect("validated array capacity");
            }
            values.into_boxed()
        },
        last: get_last(input, sizes)?,
    })
}
#[must_use]
pub fn encode_pattern(value: &Pattern, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_pattern(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_pattern(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_pattern(bytes: &[u8], sizes: &Sizes) -> Option<Pattern> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_pattern(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_pattern(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = u64::try_from(size_of::<Box<[u8]>>())
            .ok()?
            .checked_add(Some(u64::from(sizes.name_bytes))?)?
            .checked_mul(u64::from(sizes.entries))?;
        total = total.checked_add(field)?;
        let field = heap_last(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Grant {
    pub connector: u16,
    pub kind: u16,
    pub pattern: Pattern,
}
pub(crate) fn put_grant(out: &mut Encoder, value: &Grant, sizes: &Sizes) -> Option<()> {
    let connector = &value.connector;
    out.u16(*connector)?;
    let kind = &value.kind;
    out.u16(*kind)?;
    let pattern = &value.pattern;
    put_pattern(out, pattern, sizes)?;
    Some(())
}
pub(crate) fn get_grant(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Grant> {
    Some(Grant { connector: input.u16()?, kind: input.u16()?, pattern: get_pattern(input, sizes)? })
}
#[must_use]
pub fn encode_grant(value: &Grant, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_grant(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_grant(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_grant(bytes: &[u8], sizes: &Sizes) -> Option<Grant> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_grant(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_grant(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = heap_pattern(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Delegation {
    pub kinds: Box<[ExecutorKind]>,
    pub tasks: u32,
    pub depth: u32,
}
pub(crate) fn put_delegation(out: &mut Encoder, value: &Delegation, sizes: &Sizes) -> Option<()> {
    let kinds = &value.kinds;
    let count_7 = u32::try_from(kinds.len()).ok()?;
    if count_7 > sizes.entries {
        return None;
    }
    out.u32(count_7)?;
    for element in kinds {
        put_executor_kind(out, element, sizes)?;
    }
    let tasks = &value.tasks;
    out.u32(*tasks)?;
    let depth = &value.depth;
    out.u32(*depth)?;
    Some(())
}
pub(crate) fn get_delegation(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Delegation> {
    Some(Delegation {
        kinds: {
            let count = p::count(input, sizes.entries, 5)?;
            let mut values = List::with_capacity(count);
            for _ in 0..count {
                let value = get_executor_kind(input, sizes)?;
                values.push(value).expect("validated array capacity");
            }
            values.into_boxed()
        },
        tasks: input.u32()?,
        depth: input.u32()?,
    })
}
#[must_use]
pub fn encode_delegation(value: &Delegation, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_delegation(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_delegation(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_delegation(bytes: &[u8], sizes: &Sizes) -> Option<Delegation> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_delegation(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_delegation(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = u64::try_from(size_of::<ExecutorKind>())
            .ok()?
            .checked_add(heap_executor_kind(sizes)?)?
            .checked_mul(u64::from(sizes.entries))?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Authority {
    pub tools: u64,
    pub grants: Box<[Grant]>,
    pub delegation: Delegation,
    pub spend: u64,
    pub deadline: Option<u64>,
    pub notes: u8,
}
pub(crate) fn put_authority(out: &mut Encoder, value: &Authority, sizes: &Sizes) -> Option<()> {
    let tools = &value.tools;
    out.u64(*tools)?;
    let grants = &value.grants;
    let count_8 = u32::try_from(grants.len()).ok()?;
    if count_8 > sizes.entries {
        return None;
    }
    out.u32(count_8)?;
    for element in grants {
        put_grant(out, element, sizes)?;
    }
    let delegation = &value.delegation;
    put_delegation(out, delegation, sizes)?;
    let spend = &value.spend;
    out.u64(*spend)?;
    let deadline = &value.deadline;
    match deadline {
        Some(element) => {
            out.u8(1)?;
            out.u64(*element)?;
        }
        None => {
            out.u8(0)?;
        }
    }
    let notes = &value.notes;
    out.u8(*notes)?;
    Some(())
}
pub(crate) fn get_authority(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Authority> {
    Some(Authority {
        tools: input.u64()?,
        grants: {
            let count = p::count(input, sizes.entries, 9)?;
            let mut values = List::with_capacity(count);
            for _ in 0..count {
                let value = get_grant(input, sizes)?;
                values.push(value).expect("validated array capacity");
            }
            values.into_boxed()
        },
        delegation: get_delegation(input, sizes)?,
        spend: input.u64()?,
        deadline: match input.u8()? {
            0 => None,
            1 => Some(input.u64()?),
            _ => return None,
        },
        notes: input.u8()?,
    })
}
#[must_use]
pub fn encode_authority(value: &Authority, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_authority(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_authority(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_authority(bytes: &[u8], sizes: &Sizes) -> Option<Authority> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_authority(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_authority(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = u64::try_from(size_of::<Grant>())
            .ok()?
            .checked_add(heap_grant(sizes)?)?
            .checked_mul(u64::from(sizes.entries))?;
        total = total.checked_add(field)?;
        let field = heap_delegation(sizes)?;
        total = total.checked_add(field)?;
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

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Resources {
    pub read: Box<[Resource]>,
    pub write: Box<[Resource]>,
}
pub(crate) fn put_resources(out: &mut Encoder, value: &Resources, sizes: &Sizes) -> Option<()> {
    let read = &value.read;
    let count_9 = u32::try_from(read.len()).ok()?;
    if count_9 > sizes.entries {
        return None;
    }
    out.u32(count_9)?;
    for element in read {
        put_resource(out, element, sizes)?;
    }
    let write = &value.write;
    let count_10 = u32::try_from(write.len()).ok()?;
    if count_10 > sizes.entries {
        return None;
    }
    out.u32(count_10)?;
    for element in write {
        put_resource(out, element, sizes)?;
    }
    Some(())
}
pub(crate) fn get_resources(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Resources> {
    Some(Resources {
        read: {
            let count = p::count(input, sizes.entries, 5)?;
            let mut values = List::with_capacity(count);
            for _ in 0..count {
                let value = get_resource(input, sizes)?;
                values.push(value).expect("validated array capacity");
            }
            values.into_boxed()
        },
        write: {
            let count = p::count(input, sizes.entries, 5)?;
            let mut values = List::with_capacity(count);
            for _ in 0..count {
                let value = get_resource(input, sizes)?;
                values.push(value).expect("validated array capacity");
            }
            values.into_boxed()
        },
    })
}
#[must_use]
pub fn encode_resources(value: &Resources, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_resources(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_resources(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_resources(bytes: &[u8], sizes: &Sizes) -> Option<Resources> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_resources(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_resources(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = u64::try_from(size_of::<Resource>())
            .ok()?
            .checked_add(heap_resource(sizes)?)?
            .checked_mul(u64::from(sizes.entries))?;
        total = total.checked_add(field)?;
        let field = u64::try_from(size_of::<Resource>())
            .ok()?
            .checked_add(heap_resource(sizes)?)?
            .checked_mul(u64::from(sizes.entries))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Wake {
    pub own: WakeClass,
    pub related: WakeClass,
    pub subscribed: WakeClass,
    pub messages: WakeClass,
    pub count: u32,
    pub age: Option<Duration>,
}
pub(crate) fn put_wake(out: &mut Encoder, value: &Wake, sizes: &Sizes) -> Option<()> {
    let own = &value.own;
    put_wake_class(out, own, sizes)?;
    let related = &value.related;
    put_wake_class(out, related, sizes)?;
    let subscribed = &value.subscribed;
    put_wake_class(out, subscribed, sizes)?;
    let messages = &value.messages;
    put_wake_class(out, messages, sizes)?;
    let count = &value.count;
    out.u32(*count)?;
    let age = &value.age;
    match age {
        Some(element) => {
            out.u8(1)?;
            out.u64(element.as_nanos())?;
        }
        None => {
            out.u8(0)?;
        }
    }
    Some(())
}
pub(crate) fn get_wake(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Wake> {
    Some(Wake {
        own: get_wake_class(input, sizes)?,
        related: get_wake_class(input, sizes)?,
        subscribed: get_wake_class(input, sizes)?,
        messages: get_wake_class(input, sizes)?,
        count: input.u32()?,
        age: match input.u8()? {
            0 => None,
            1 => Some(Duration::from_nanos(input.u64()?)),
            _ => return None,
        },
    })
}
#[must_use]
pub fn encode_wake(value: &Wake, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_wake(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_wake(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_wake(bytes: &[u8], sizes: &Sizes) -> Option<Wake> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_wake(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_wake(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = heap_wake_class(sizes)?;
        total = total.checked_add(field)?;
        let field = heap_wake_class(sizes)?;
        total = total.checked_add(field)?;
        let field = heap_wake_class(sizes)?;
        total = total.checked_add(field)?;
        let field = heap_wake_class(sizes)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Spec {
    pub words: Box<[u8]>,
    pub resources: Resources,
    pub inputs: Box<[u64]>,
}
pub(crate) fn put_spec(out: &mut Encoder, value: &Spec, sizes: &Sizes) -> Option<()> {
    let words = &value.words;
    out.bytes(words, sizes.detail)?;
    let resources = &value.resources;
    put_resources(out, resources, sizes)?;
    let inputs = &value.inputs;
    let count_11 = u32::try_from(inputs.len()).ok()?;
    if count_11 > sizes.entries {
        return None;
    }
    out.u32(count_11)?;
    for element in inputs {
        out.u64(*element)?;
    }
    Some(())
}
pub(crate) fn get_spec(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Spec> {
    Some(Spec {
        words: p::bytes(input, sizes.detail)?,
        resources: get_resources(input, sizes)?,
        inputs: {
            let count = p::count(input, sizes.entries, 8)?;
            let mut values = List::with_capacity(count);
            for _ in 0..count {
                let value = input.u64()?;
                values.push(value).expect("validated array capacity");
            }
            values.into_boxed()
        },
    })
}
#[must_use]
pub fn encode_spec(value: &Spec, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_spec(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_spec(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_spec(bytes: &[u8], sizes: &Sizes) -> Option<Spec> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_spec(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_spec(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        let field = heap_resources(sizes)?;
        total = total.checked_add(field)?;
        let field =
            u64::try_from(size_of::<u64>()).ok()?.checked_add(Some(0_u64)?)?.checked_mul(u64::from(sizes.entries))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NewTask {
    pub spec: Spec,
    pub contract: TaskContract,
    pub executor: Executor,
    pub authority: Authority,
    pub dependencies: Box<[Dependency]>,
    pub references: Box<[u64]>,
    pub wake: Wake,
    pub subscriptions: Box<[Topic]>,
    pub tracked: bool,
    pub priority: u32,
}
pub(crate) fn put_new_task(out: &mut Encoder, value: &NewTask, sizes: &Sizes) -> Option<()> {
    let spec = &value.spec;
    put_spec(out, spec, sizes)?;
    let contract = &value.contract;
    put_task_contract(out, contract, sizes)?;
    let executor = &value.executor;
    put_executor(out, executor, sizes)?;
    let authority = &value.authority;
    put_authority(out, authority, sizes)?;
    let dependencies = &value.dependencies;
    let count_12 = u32::try_from(dependencies.len()).ok()?;
    if count_12 > sizes.entries {
        return None;
    }
    out.u32(count_12)?;
    for element in dependencies {
        put_dependency(out, element, sizes)?;
    }
    let references = &value.references;
    let count_13 = u32::try_from(references.len()).ok()?;
    if count_13 > sizes.entries {
        return None;
    }
    out.u32(count_13)?;
    for element in references {
        out.u64(*element)?;
    }
    let wake = &value.wake;
    put_wake(out, wake, sizes)?;
    let subscriptions = &value.subscriptions;
    let count_14 = u32::try_from(subscriptions.len()).ok()?;
    if count_14 > sizes.entries {
        return None;
    }
    out.u32(count_14)?;
    for element in subscriptions {
        put_topic(out, element, sizes)?;
    }
    let tracked = &value.tracked;
    out.bool(*tracked)?;
    let priority = &value.priority;
    out.u32(*priority)?;
    Some(())
}
pub(crate) fn get_new_task(input: &mut Reader<'_>, sizes: &Sizes) -> Option<NewTask> {
    Some(NewTask {
        spec: get_spec(input, sizes)?,
        contract: get_task_contract(input, sizes)?,
        executor: get_executor(input, sizes)?,
        authority: get_authority(input, sizes)?,
        dependencies: {
            let count = p::count(input, sizes.entries, 5)?;
            let mut values = List::with_capacity(count);
            for _ in 0..count {
                let value = get_dependency(input, sizes)?;
                values.push(value).expect("validated array capacity");
            }
            values.into_boxed()
        },
        references: {
            let count = p::count(input, sizes.entries, 8)?;
            let mut values = List::with_capacity(count);
            for _ in 0..count {
                let value = input.u64()?;
                values.push(value).expect("validated array capacity");
            }
            values.into_boxed()
        },
        wake: get_wake(input, sizes)?,
        subscriptions: {
            let count = p::count(input, sizes.entries, 7)?;
            let mut values = List::with_capacity(count);
            for _ in 0..count {
                let value = get_topic(input, sizes)?;
                values.push(value).expect("validated array capacity");
            }
            values.into_boxed()
        },
        tracked: p::boolean(input)?,
        priority: input.u32()?,
    })
}
#[must_use]
pub fn encode_new_task(value: &NewTask, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_new_task(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_new_task(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_new_task(bytes: &[u8], sizes: &Sizes) -> Option<NewTask> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_new_task(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_new_task(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = heap_spec(sizes)?;
        total = total.checked_add(field)?;
        let field = heap_task_contract(sizes)?;
        total = total.checked_add(field)?;
        let field = heap_executor(sizes)?;
        total = total.checked_add(field)?;
        let field = heap_authority(sizes)?;
        total = total.checked_add(field)?;
        let field = u64::try_from(size_of::<Dependency>())
            .ok()?
            .checked_add(heap_dependency(sizes)?)?
            .checked_mul(u64::from(sizes.entries))?;
        total = total.checked_add(field)?;
        let field =
            u64::try_from(size_of::<u64>()).ok()?.checked_add(Some(0_u64)?)?.checked_mul(u64::from(sizes.entries))?;
        total = total.checked_add(field)?;
        let field = heap_wake(sizes)?;
        total = total.checked_add(field)?;
        let field = u64::try_from(size_of::<Topic>())
            .ok()?
            .checked_add(heap_topic(sizes)?)?
            .checked_mul(u64::from(sizes.entries))?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Amendment {
    pub spec: Option<Spec>,
    pub wake: Option<Wake>,
    pub remove_dependencies: Box<[u64]>,
    pub authority: Option<Authority>,
    pub instructions: Option<Box<[u8]>>,
    pub procedure: Option<ProcedureParameters>,
}
pub(crate) fn put_amendment(out: &mut Encoder, value: &Amendment, sizes: &Sizes) -> Option<()> {
    let spec = &value.spec;
    match spec {
        Some(element) => {
            out.u8(1)?;
            put_spec(out, element, sizes)?;
        }
        None => {
            out.u8(0)?;
        }
    }
    let wake = &value.wake;
    match wake {
        Some(element) => {
            out.u8(1)?;
            put_wake(out, element, sizes)?;
        }
        None => {
            out.u8(0)?;
        }
    }
    let remove_dependencies = &value.remove_dependencies;
    let count_15 = u32::try_from(remove_dependencies.len()).ok()?;
    if count_15 > sizes.entries {
        return None;
    }
    out.u32(count_15)?;
    for element in remove_dependencies {
        out.u64(*element)?;
    }
    let authority = &value.authority;
    match authority {
        Some(element) => {
            out.u8(1)?;
            put_authority(out, element, sizes)?;
        }
        None => {
            out.u8(0)?;
        }
    }
    let instructions = &value.instructions;
    match instructions {
        Some(element) => {
            out.u8(1)?;
            out.bytes(element, sizes.detail)?;
        }
        None => {
            out.u8(0)?;
        }
    }
    let procedure = &value.procedure;
    match procedure {
        Some(element) => {
            out.u8(1)?;
            put_procedure_parameters(out, element, sizes)?;
        }
        None => {
            out.u8(0)?;
        }
    }
    Some(())
}
pub(crate) fn get_amendment(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Amendment> {
    Some(Amendment {
        spec: match input.u8()? {
            0 => None,
            1 => Some(get_spec(input, sizes)?),
            _ => return None,
        },
        wake: match input.u8()? {
            0 => None,
            1 => Some(get_wake(input, sizes)?),
            _ => return None,
        },
        remove_dependencies: {
            let count = p::count(input, sizes.entries, 8)?;
            let mut values = List::with_capacity(count);
            for _ in 0..count {
                let value = input.u64()?;
                values.push(value).expect("validated array capacity");
            }
            values.into_boxed()
        },
        authority: match input.u8()? {
            0 => None,
            1 => Some(get_authority(input, sizes)?),
            _ => return None,
        },
        instructions: match input.u8()? {
            0 => None,
            1 => Some(p::bytes(input, sizes.detail)?),
            _ => return None,
        },
        procedure: match input.u8()? {
            0 => None,
            1 => Some(get_procedure_parameters(input, sizes)?),
            _ => return None,
        },
    })
}
#[must_use]
pub fn encode_amendment(value: &Amendment, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_amendment(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_amendment(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_amendment(bytes: &[u8], sizes: &Sizes) -> Option<Amendment> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_amendment(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_amendment(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = heap_spec(sizes)?;
        total = total.checked_add(field)?;
        let field = heap_wake(sizes)?;
        total = total.checked_add(field)?;
        let field =
            u64::try_from(size_of::<u64>()).ok()?.checked_add(Some(0_u64)?)?.checked_mul(u64::from(sizes.entries))?;
        total = total.checked_add(field)?;
        let field = heap_authority(sizes)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        let field = heap_procedure_parameters(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Note {
    pub name: Box<[u8]>,
    pub revision: u64,
    pub words: Box<[u8]>,
}
pub(crate) fn put_note(out: &mut Encoder, value: &Note, sizes: &Sizes) -> Option<()> {
    let name = &value.name;
    out.bytes(name, sizes.name_bytes)?;
    let revision = &value.revision;
    out.u64(*revision)?;
    let words = &value.words;
    out.bytes(words, sizes.detail)?;
    Some(())
}
pub(crate) fn get_note(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Note> {
    Some(Note {
        name: p::bytes(input, sizes.name_bytes)?,
        revision: input.u64()?,
        words: p::bytes(input, sizes.detail)?,
    })
}
#[must_use]
pub fn encode_note(value: &Note, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_note(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_note(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_note(bytes: &[u8], sizes: &Sizes) -> Option<Note> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_note(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_note(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(u64::from(sizes.name_bytes))?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Field {
    pub name: Box<[u8]>,
    pub value: Box<[u8]>,
}
pub(crate) fn put_field(out: &mut Encoder, value: &Field, sizes: &Sizes) -> Option<()> {
    let name = &value.name;
    out.bytes(name, sizes.name_bytes)?;
    let value = &value.value;
    out.bytes(value, sizes.detail)?;
    Some(())
}
pub(crate) fn get_field(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Field> {
    Some(Field { name: p::bytes(input, sizes.name_bytes)?, value: p::bytes(input, sizes.detail)? })
}
#[must_use]
pub fn encode_field(value: &Field, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_field(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_field(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_field(bytes: &[u8], sizes: &Sizes) -> Option<Field> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_field(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_field(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(u64::from(sizes.name_bytes))?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Inbound {
    pub number: u64,
    pub message: Message,
}
pub(crate) fn put_inbound(out: &mut Encoder, value: &Inbound, sizes: &Sizes) -> Option<()> {
    let number = &value.number;
    out.u64(*number)?;
    let message = &value.message;
    put_message(out, message, sizes)?;
    Some(())
}
pub(crate) fn get_inbound(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Inbound> {
    Some(Inbound { number: input.u64()?, message: get_message(input, sizes)? })
}
#[must_use]
pub fn encode_inbound(value: &Inbound, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_inbound(&mut out, value, sizes)?;
    if out.length() > sizes.inbound {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_inbound(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_inbound(bytes: &[u8], sizes: &Sizes) -> Option<Inbound> {
    if u32::try_from(bytes.len()).ok()? > sizes.inbound {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_inbound(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_inbound(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = heap_message(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct File {
    pub path: Box<[u8]>,
    pub content: Box<[u8]>,
}
pub(crate) fn put_file(out: &mut Encoder, value: &File, sizes: &Sizes) -> Option<()> {
    let path = &value.path;
    out.bytes(path, sizes.name_bytes)?;
    let content = &value.content;
    out.bytes(content, sizes.detail)?;
    Some(())
}
pub(crate) fn get_file(input: &mut Reader<'_>, sizes: &Sizes) -> Option<File> {
    Some(File { path: p::bytes(input, sizes.name_bytes)?, content: p::bytes(input, sizes.detail)? })
}
#[must_use]
pub fn encode_file(value: &File, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_file(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_file(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_file(bytes: &[u8], sizes: &Sizes) -> Option<File> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_file(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_file(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(u64::from(sizes.name_bytes))?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CiStatus {
    pub name: Box<[u8]>,
    pub passed: bool,
    pub pending: bool,
}
pub(crate) fn put_ci_status(out: &mut Encoder, value: &CiStatus, sizes: &Sizes) -> Option<()> {
    let name = &value.name;
    out.bytes(name, sizes.name_bytes)?;
    let passed = &value.passed;
    out.bool(*passed)?;
    let pending = &value.pending;
    out.bool(*pending)?;
    Some(())
}
pub(crate) fn get_ci_status(input: &mut Reader<'_>, sizes: &Sizes) -> Option<CiStatus> {
    Some(CiStatus { name: p::bytes(input, sizes.name_bytes)?, passed: p::boolean(input)?, pending: p::boolean(input)? })
}
#[must_use]
pub fn encode_ci_status(value: &CiStatus, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_ci_status(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_ci_status(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_ci_status(bytes: &[u8], sizes: &Sizes) -> Option<CiStatus> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_ci_status(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_ci_status(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(u64::from(sizes.name_bytes))?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Comment {
    pub number: u64,
    pub words: Box<[u8]>,
}
pub(crate) fn put_comment(out: &mut Encoder, value: &Comment, sizes: &Sizes) -> Option<()> {
    let number = &value.number;
    out.u64(*number)?;
    let words = &value.words;
    out.bytes(words, sizes.detail)?;
    Some(())
}
pub(crate) fn get_comment(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Comment> {
    Some(Comment { number: input.u64()?, words: p::bytes(input, sizes.detail)? })
}
#[must_use]
pub fn encode_comment(value: &Comment, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_comment(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_comment(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_comment(bytes: &[u8], sizes: &Sizes) -> Option<Comment> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_comment(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_comment(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Turn {
    pub version: u16,
    pub body: Box<[u8]>,
    pub spent: u64,
    pub read: Option<u64>,
}
pub(crate) fn put_turn(out: &mut Encoder, value: &Turn, sizes: &Sizes) -> Option<()> {
    let version = &value.version;
    out.u16(*version)?;
    let body = &value.body;
    out.bytes(body, sizes.turn)?;
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
    Some(())
}
pub(crate) fn get_turn(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Turn> {
    Some(Turn {
        version: input.u16()?,
        body: p::bytes(input, sizes.turn)?,
        spent: input.u64()?,
        read: match input.u8()? {
            0 => None,
            1 => Some(input.u64()?),
            _ => return None,
        },
    })
}
#[must_use]
pub fn encode_turn(value: &Turn, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_turn(&mut out, value, sizes)?;
    if out.length() > sizes.turn {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_turn(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_turn(bytes: &[u8], sizes: &Sizes) -> Option<Turn> {
    if u32::try_from(bytes.len()).ok()? > sizes.turn {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_turn(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_turn(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.turn))?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CommittedCall {
    pub call: u64,
    pub ask: Call,
    pub answer: Served,
}
pub(crate) fn put_committed_call(out: &mut Encoder, value: &CommittedCall, sizes: &Sizes) -> Option<()> {
    let call = &value.call;
    out.u64(*call)?;
    let ask = &value.ask;
    put_call(out, ask, sizes)?;
    let answer = &value.answer;
    put_served(out, answer, sizes)?;
    Some(())
}
pub(crate) fn get_committed_call(input: &mut Reader<'_>, sizes: &Sizes) -> Option<CommittedCall> {
    Some(CommittedCall { call: input.u64()?, ask: get_call(input, sizes)?, answer: get_served(input, sizes)? })
}
#[must_use]
pub fn encode_committed_call(value: &CommittedCall, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_committed_call(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_committed_call(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_committed_call(bytes: &[u8], sizes: &Sizes) -> Option<CommittedCall> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_committed_call(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_committed_call(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = heap_call(sizes)?;
        total = total.checked_add(field)?;
        let field = heap_served(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Transcript {
    pub turns: Box<[Turn]>,
    pub calls: Box<[CommittedCall]>,
}
pub(crate) fn put_transcript(out: &mut Encoder, value: &Transcript, sizes: &Sizes) -> Option<()> {
    let turns = &value.turns;
    let count_16 = u32::try_from(turns.len()).ok()?;
    if count_16 > sizes.entries {
        return None;
    }
    out.u32(count_16)?;
    for element in turns {
        put_turn(out, element, sizes)?;
    }
    let calls = &value.calls;
    let count_17 = u32::try_from(calls.len()).ok()?;
    if count_17 > sizes.entries {
        return None;
    }
    out.u32(count_17)?;
    for element in calls {
        put_committed_call(out, element, sizes)?;
    }
    Some(())
}
pub(crate) fn get_transcript(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Transcript> {
    Some(Transcript {
        turns: {
            let count = p::count(input, sizes.entries, 15)?;
            let mut values = List::with_capacity(count);
            for _ in 0..count {
                let value = get_turn(input, sizes)?;
                values.push(value).expect("validated array capacity");
            }
            values.into_boxed()
        },
        calls: {
            let count = p::count(input, sizes.entries, 14)?;
            let mut values = List::with_capacity(count);
            for _ in 0..count {
                let value = get_committed_call(input, sizes)?;
                values.push(value).expect("validated array capacity");
            }
            values.into_boxed()
        },
    })
}
#[must_use]
pub fn encode_transcript(value: &Transcript, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_transcript(&mut out, value, sizes)?;
    if out.length() > sizes.transcript {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_transcript(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_transcript(bytes: &[u8], sizes: &Sizes) -> Option<Transcript> {
    if u32::try_from(bytes.len()).ok()? > sizes.transcript {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_transcript(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_transcript(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = u64::try_from(size_of::<Turn>())
            .ok()?
            .checked_add(heap_turn(sizes)?)?
            .checked_mul(u64::from(sizes.entries))?;
        total = total.checked_add(field)?;
        let field = u64::try_from(size_of::<CommittedCall>())
            .ok()?
            .checked_add(heap_committed_call(sizes)?)?
            .checked_mul(u64::from(sizes.entries))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ModelKind {
    Main,
    Subagent,
}
pub(crate) fn put_model_kind(out: &mut Encoder, value: &ModelKind, _sizes: &Sizes) -> Option<()> {
    match value {
        ModelKind::Main => {
            out.u8(0)?;
        }
        ModelKind::Subagent => {
            out.u8(1)?;
        }
    }
    Some(())
}
pub(crate) fn get_model_kind(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<ModelKind> {
    match input.u8()? {
        0 => Some(ModelKind::Main),
        1 => Some(ModelKind::Subagent),
        _ => None,
    }
}
#[must_use]
pub fn encode_model_kind(value: &ModelKind, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_model_kind(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_model_kind(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_model_kind(bytes: &[u8], sizes: &Sizes) -> Option<ModelKind> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_model_kind(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_model_kind(_sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SectionKind {
    Task,
    Inputs,
    Delegates,
    Inbox,
    Notes,
    Resources,
    Calls,
}
pub(crate) fn put_section_kind(out: &mut Encoder, value: &SectionKind, _sizes: &Sizes) -> Option<()> {
    match value {
        SectionKind::Task => {
            out.u8(0)?;
        }
        SectionKind::Inputs => {
            out.u8(1)?;
        }
        SectionKind::Delegates => {
            out.u8(2)?;
        }
        SectionKind::Inbox => {
            out.u8(3)?;
        }
        SectionKind::Notes => {
            out.u8(4)?;
        }
        SectionKind::Resources => {
            out.u8(5)?;
        }
        SectionKind::Calls => {
            out.u8(6)?;
        }
    }
    Some(())
}
pub(crate) fn get_section_kind(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<SectionKind> {
    match input.u8()? {
        0 => Some(SectionKind::Task),
        1 => Some(SectionKind::Inputs),
        2 => Some(SectionKind::Delegates),
        3 => Some(SectionKind::Inbox),
        4 => Some(SectionKind::Notes),
        5 => Some(SectionKind::Resources),
        6 => Some(SectionKind::Calls),
        _ => None,
    }
}
#[must_use]
pub fn encode_section_kind(value: &SectionKind, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_section_kind(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_section_kind(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_section_kind(bytes: &[u8], sizes: &Sizes) -> Option<SectionKind> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_section_kind(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_section_kind(_sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Contract {
    Report { most: u32 },
    Verdict { verdicts: Box<[VerdictRule]> },
    Change { checks: bool },
}
pub(crate) fn put_contract(out: &mut Encoder, value: &Contract, sizes: &Sizes) -> Option<()> {
    match value {
        Contract::Report { most } => {
            out.u8(0)?;
            out.u32(*most)?;
        }
        Contract::Verdict { verdicts } => {
            out.u8(1)?;
            let count_18 = u32::try_from(verdicts.len()).ok()?;
            if count_18 > sizes.entries {
                return None;
            }
            out.u32(count_18)?;
            for element in verdicts {
                put_verdict_rule(out, element, sizes)?;
            }
        }
        Contract::Change { checks } => {
            out.u8(2)?;
            out.bool(*checks)?;
        }
    }
    Some(())
}
pub(crate) fn get_contract(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Contract> {
    match input.u8()? {
        0 => Some(Contract::Report { most: input.u32()? }),
        1 => Some(Contract::Verdict {
            verdicts: {
                let count = p::count(input, sizes.entries, 20)?;
                let mut values = List::with_capacity(count);
                for _ in 0..count {
                    let value = get_verdict_rule(input, sizes)?;
                    values.push(value).expect("validated array capacity");
                }
                values.into_boxed()
            },
        }),
        2 => Some(Contract::Change { checks: p::boolean(input)? }),
        _ => None,
    }
}
#[must_use]
pub fn encode_contract(value: &Contract, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_contract(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_contract(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_contract(bytes: &[u8], sizes: &Sizes) -> Option<Contract> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_contract(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_contract(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = u64::try_from(size_of::<VerdictRule>())
            .ok()?
            .checked_add(heap_verdict_rule(sizes)?)?
            .checked_mul(u64::from(sizes.entries))?;
        total = total.checked_add(field)?;
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

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Last {
    None,
    Exact { segment: Box<[u8]> },
    Open { prefix: Box<[u8]> },
}
pub(crate) fn put_last(out: &mut Encoder, value: &Last, sizes: &Sizes) -> Option<()> {
    match value {
        Last::None => {
            out.u8(0)?;
        }
        Last::Exact { segment } => {
            out.u8(1)?;
            out.bytes(segment, sizes.name_bytes)?;
        }
        Last::Open { prefix } => {
            out.u8(2)?;
            out.bytes(prefix, sizes.name_bytes)?;
        }
    }
    Some(())
}
pub(crate) fn get_last(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Last> {
    match input.u8()? {
        0 => Some(Last::None),
        1 => Some(Last::Exact { segment: p::bytes(input, sizes.name_bytes)? }),
        2 => Some(Last::Open { prefix: p::bytes(input, sizes.name_bytes)? }),
        _ => None,
    }
}
#[must_use]
pub fn encode_last(value: &Last, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_last(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_last(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_last(bytes: &[u8], sizes: &Sizes) -> Option<Last> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_last(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_last(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let total = 0_u64;
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
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ExecutorKind {
    Charter { kind: u32 },
    Procedure { kind: u32 },
    Role { kind: u32 },
}
pub(crate) fn put_executor_kind(out: &mut Encoder, value: &ExecutorKind, _sizes: &Sizes) -> Option<()> {
    match value {
        ExecutorKind::Charter { kind } => {
            out.u8(0)?;
            out.u32(*kind)?;
        }
        ExecutorKind::Procedure { kind } => {
            out.u8(1)?;
            out.u32(*kind)?;
        }
        ExecutorKind::Role { kind } => {
            out.u8(2)?;
            out.u32(*kind)?;
        }
    }
    Some(())
}
pub(crate) fn get_executor_kind(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<ExecutorKind> {
    match input.u8()? {
        0 => Some(ExecutorKind::Charter { kind: input.u32()? }),
        1 => Some(ExecutorKind::Procedure { kind: input.u32()? }),
        2 => Some(ExecutorKind::Role { kind: input.u32()? }),
        _ => None,
    }
}
#[must_use]
pub fn encode_executor_kind(value: &ExecutorKind, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_executor_kind(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_executor_kind(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_executor_kind(bytes: &[u8], sizes: &Sizes) -> Option<ExecutorKind> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_executor_kind(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_executor_kind(_sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
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
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum BranchRole {
    Change,
    Saved,
    Runs,
}
pub(crate) fn put_branch_role(out: &mut Encoder, value: &BranchRole, _sizes: &Sizes) -> Option<()> {
    match value {
        BranchRole::Change => {
            out.u8(0)?;
        }
        BranchRole::Saved => {
            out.u8(1)?;
        }
        BranchRole::Runs => {
            out.u8(2)?;
        }
    }
    Some(())
}
pub(crate) fn get_branch_role(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<BranchRole> {
    match input.u8()? {
        0 => Some(BranchRole::Change),
        1 => Some(BranchRole::Saved),
        2 => Some(BranchRole::Runs),
        _ => None,
    }
}
#[must_use]
pub fn encode_branch_role(value: &BranchRole, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_branch_role(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_branch_role(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_branch_role(bytes: &[u8], sizes: &Sizes) -> Option<BranchRole> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_branch_role(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_branch_role(_sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Resource {
    Named { name: Name },
    Own { repository: Name, role: BranchRole },
}
pub(crate) fn put_resource(out: &mut Encoder, value: &Resource, sizes: &Sizes) -> Option<()> {
    match value {
        Resource::Named { name } => {
            out.u8(0)?;
            put_name(out, name, sizes)?;
        }
        Resource::Own { repository, role } => {
            out.u8(1)?;
            put_name(out, repository, sizes)?;
            put_branch_role(out, role, sizes)?;
        }
    }
    Some(())
}
pub(crate) fn get_resource(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Resource> {
    match input.u8()? {
        0 => Some(Resource::Named { name: get_name(input, sizes)? }),
        1 => Some(Resource::Own { repository: get_name(input, sizes)?, role: get_branch_role(input, sizes)? }),
        _ => None,
    }
}
#[must_use]
pub fn encode_resource(value: &Resource, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_resource(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_resource(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_resource(bytes: &[u8], sizes: &Sizes) -> Option<Resource> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_resource(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_resource(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = heap_name(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = heap_name(sizes)?;
        total = total.checked_add(field)?;
        let field = heap_branch_role(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Dependency {
    Task { number: u64 },
    Batch { index: u32 },
}
pub(crate) fn put_dependency(out: &mut Encoder, value: &Dependency, _sizes: &Sizes) -> Option<()> {
    match value {
        Dependency::Task { number } => {
            out.u8(0)?;
            out.u64(*number)?;
        }
        Dependency::Batch { index } => {
            out.u8(1)?;
            out.u32(*index)?;
        }
    }
    Some(())
}
pub(crate) fn get_dependency(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<Dependency> {
    match input.u8()? {
        0 => Some(Dependency::Task { number: input.u64()? }),
        1 => Some(Dependency::Batch { index: input.u32()? }),
        _ => None,
    }
}
#[must_use]
pub fn encode_dependency(value: &Dependency, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_dependency(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_dependency(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_dependency(bytes: &[u8], sizes: &Sizes) -> Option<Dependency> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_dependency(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_dependency(_sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
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

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum WakeClass {
    All,
    Critical,
    Never,
}
pub(crate) fn put_wake_class(out: &mut Encoder, value: &WakeClass, _sizes: &Sizes) -> Option<()> {
    match value {
        WakeClass::All => {
            out.u8(0)?;
        }
        WakeClass::Critical => {
            out.u8(1)?;
        }
        WakeClass::Never => {
            out.u8(2)?;
        }
    }
    Some(())
}
pub(crate) fn get_wake_class(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<WakeClass> {
    match input.u8()? {
        0 => Some(WakeClass::All),
        1 => Some(WakeClass::Critical),
        2 => Some(WakeClass::Never),
        _ => None,
    }
}
#[must_use]
pub fn encode_wake_class(value: &WakeClass, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_wake_class(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_wake_class(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_wake_class(bytes: &[u8], sizes: &Sizes) -> Option<WakeClass> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_wake_class(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_wake_class(_sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Topic {
    Task { task: u64 },
    Resource { connector: u16, resource: Name },
    Timer { at: u64, repeat: Option<Duration> },
}
pub(crate) fn put_topic(out: &mut Encoder, value: &Topic, sizes: &Sizes) -> Option<()> {
    match value {
        Topic::Task { task } => {
            out.u8(0)?;
            out.u64(*task)?;
        }
        Topic::Resource { connector, resource } => {
            out.u8(1)?;
            out.u16(*connector)?;
            put_name(out, resource, sizes)?;
        }
        Topic::Timer { at, repeat } => {
            out.u8(2)?;
            out.u64(*at)?;
            match repeat {
                Some(element) => {
                    out.u8(1)?;
                    out.u64(element.as_nanos())?;
                }
                None => {
                    out.u8(0)?;
                }
            }
        }
    }
    Some(())
}
pub(crate) fn get_topic(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Topic> {
    match input.u8()? {
        0 => Some(Topic::Task { task: input.u64()? }),
        1 => Some(Topic::Resource { connector: input.u16()?, resource: get_name(input, sizes)? }),
        2 => Some(Topic::Timer {
            at: input.u64()?,
            repeat: match input.u8()? {
                0 => None,
                1 => Some(Duration::from_nanos(input.u64()?)),
                _ => return None,
            },
        }),
        _ => None,
    }
}
#[must_use]
pub fn encode_topic(value: &Topic, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_topic(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_topic(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_topic(bytes: &[u8], sizes: &Sizes) -> Option<Topic> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_topic(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_topic(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = heap_name(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ProcedureParameters {
    Land { repository: Name, branch: Box<[u8]>, base: Box<[u8]>, head: [u8; 32] },
    Watch { topic: Topic },
}
pub(crate) fn put_procedure_parameters(out: &mut Encoder, value: &ProcedureParameters, sizes: &Sizes) -> Option<()> {
    match value {
        ProcedureParameters::Land { repository, branch, base, head } => {
            out.u8(0)?;
            put_name(out, repository, sizes)?;
            out.bytes(branch, sizes.name_bytes)?;
            out.bytes(base, sizes.name_bytes)?;
            out.raw(head)?;
        }
        ProcedureParameters::Watch { topic } => {
            out.u8(1)?;
            put_topic(out, topic, sizes)?;
        }
    }
    Some(())
}
pub(crate) fn get_procedure_parameters(input: &mut Reader<'_>, sizes: &Sizes) -> Option<ProcedureParameters> {
    match input.u8()? {
        0 => Some(ProcedureParameters::Land {
            repository: get_name(input, sizes)?,
            branch: p::bytes(input, sizes.name_bytes)?,
            base: p::bytes(input, sizes.name_bytes)?,
            head: input.bytes(32)?.try_into().ok()?,
        }),
        1 => Some(ProcedureParameters::Watch { topic: get_topic(input, sizes)? }),
        _ => None,
    }
}
#[must_use]
pub fn encode_procedure_parameters(value: &ProcedureParameters, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_procedure_parameters(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_procedure_parameters(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_procedure_parameters(bytes: &[u8], sizes: &Sizes) -> Option<ProcedureParameters> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_procedure_parameters(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_procedure_parameters(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = heap_name(sizes)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.name_bytes))?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.name_bytes))?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = heap_topic(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Executor {
    Agent { charter: u32 },
    Procedure { connector: u16, kind: u32, parameters: ProcedureParameters },
    Person { role: u32, question: Box<[u8]>, choices: Box<[Box<[u8]>]> },
}
pub(crate) fn put_executor(out: &mut Encoder, value: &Executor, sizes: &Sizes) -> Option<()> {
    match value {
        Executor::Agent { charter } => {
            out.u8(0)?;
            out.u32(*charter)?;
        }
        Executor::Procedure { connector, kind, parameters } => {
            out.u8(1)?;
            out.u16(*connector)?;
            out.u32(*kind)?;
            put_procedure_parameters(out, parameters, sizes)?;
        }
        Executor::Person { role, question, choices } => {
            out.u8(2)?;
            out.u32(*role)?;
            out.bytes(question, sizes.detail)?;
            let count_19 = u32::try_from(choices.len()).ok()?;
            if count_19 > sizes.entries {
                return None;
            }
            out.u32(count_19)?;
            for element in choices {
                out.bytes(element, sizes.name_bytes)?;
            }
        }
    }
    Some(())
}
pub(crate) fn get_executor(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Executor> {
    match input.u8()? {
        0 => Some(Executor::Agent { charter: input.u32()? }),
        1 => Some(Executor::Procedure {
            connector: input.u16()?,
            kind: input.u32()?,
            parameters: get_procedure_parameters(input, sizes)?,
        }),
        2 => Some(Executor::Person {
            role: input.u32()?,
            question: p::bytes(input, sizes.detail)?,
            choices: {
                let count = p::count(input, sizes.entries, 4)?;
                let mut values = List::with_capacity(count);
                for _ in 0..count {
                    let value = p::bytes(input, sizes.name_bytes)?;
                    values.push(value).expect("validated array capacity");
                }
                values.into_boxed()
            },
        }),
        _ => None,
    }
}
#[must_use]
pub fn encode_executor(value: &Executor, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_executor(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_executor(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_executor(bytes: &[u8], sizes: &Sizes) -> Option<Executor> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_executor(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_executor(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = heap_procedure_parameters(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        let field = u64::try_from(size_of::<Box<[u8]>>())
            .ok()?
            .checked_add(Some(u64::from(sizes.name_bytes))?)?
            .checked_mul(u64::from(sizes.entries))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TaskContract {
    Run { contract: Contract },
    Choice { choices: Box<[Box<[u8]>]>, words: bool },
}
pub(crate) fn put_task_contract(out: &mut Encoder, value: &TaskContract, sizes: &Sizes) -> Option<()> {
    match value {
        TaskContract::Run { contract } => {
            out.u8(0)?;
            put_contract(out, contract, sizes)?;
        }
        TaskContract::Choice { choices, words } => {
            out.u8(1)?;
            let count_20 = u32::try_from(choices.len()).ok()?;
            if count_20 > sizes.entries {
                return None;
            }
            out.u32(count_20)?;
            for element in choices {
                out.bytes(element, sizes.name_bytes)?;
            }
            out.bool(*words)?;
        }
    }
    Some(())
}
pub(crate) fn get_task_contract(input: &mut Reader<'_>, sizes: &Sizes) -> Option<TaskContract> {
    match input.u8()? {
        0 => Some(TaskContract::Run { contract: get_contract(input, sizes)? }),
        1 => Some(TaskContract::Choice {
            choices: {
                let count = p::count(input, sizes.entries, 4)?;
                let mut values = List::with_capacity(count);
                for _ in 0..count {
                    let value = p::bytes(input, sizes.name_bytes)?;
                    values.push(value).expect("validated array capacity");
                }
                values.into_boxed()
            },
            words: p::boolean(input)?,
        }),
        _ => None,
    }
}
#[must_use]
pub fn encode_task_contract(value: &TaskContract, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_task_contract(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_task_contract(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_task_contract(bytes: &[u8], sizes: &Sizes) -> Option<TaskContract> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_task_contract(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_task_contract(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = heap_contract(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = u64::try_from(size_of::<Box<[u8]>>())
            .ok()?
            .checked_add(Some(u64::from(sizes.name_bytes))?)?
            .checked_mul(u64::from(sizes.entries))?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Decision {
    Accept,
    Reject { reason: Box<[u8]> },
    Pass,
}
pub(crate) fn put_decision(out: &mut Encoder, value: &Decision, sizes: &Sizes) -> Option<()> {
    match value {
        Decision::Accept => {
            out.u8(0)?;
        }
        Decision::Reject { reason } => {
            out.u8(1)?;
            out.bytes(reason, sizes.detail)?;
        }
        Decision::Pass => {
            out.u8(2)?;
        }
    }
    Some(())
}
pub(crate) fn get_decision(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Decision> {
    match input.u8()? {
        0 => Some(Decision::Accept),
        1 => Some(Decision::Reject { reason: p::bytes(input, sizes.detail)? }),
        2 => Some(Decision::Pass),
        _ => None,
    }
}
#[must_use]
pub fn encode_decision(value: &Decision, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_decision(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_decision(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_decision(bytes: &[u8], sizes: &Sizes) -> Option<Decision> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_decision(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_decision(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum MessageKind {
    Words,
    Question,
    Answer,
}
pub(crate) fn put_message_kind(out: &mut Encoder, value: &MessageKind, _sizes: &Sizes) -> Option<()> {
    match value {
        MessageKind::Words => {
            out.u8(0)?;
        }
        MessageKind::Question => {
            out.u8(1)?;
        }
        MessageKind::Answer => {
            out.u8(2)?;
        }
    }
    Some(())
}
pub(crate) fn get_message_kind(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<MessageKind> {
    match input.u8()? {
        0 => Some(MessageKind::Words),
        1 => Some(MessageKind::Question),
        2 => Some(MessageKind::Answer),
        _ => None,
    }
}
#[must_use]
pub fn encode_message_kind(value: &MessageKind, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_message_kind(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_message_kind(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_message_kind(bytes: &[u8], sizes: &Sizes) -> Option<MessageKind> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_message_kind(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_message_kind(_sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum NoteChange {
    Write { words: Box<[u8]> },
    Remove,
}
pub(crate) fn put_note_change(out: &mut Encoder, value: &NoteChange, sizes: &Sizes) -> Option<()> {
    match value {
        NoteChange::Write { words } => {
            out.u8(0)?;
            out.bytes(words, sizes.detail)?;
        }
        NoteChange::Remove => {
            out.u8(1)?;
        }
    }
    Some(())
}
pub(crate) fn get_note_change(input: &mut Reader<'_>, sizes: &Sizes) -> Option<NoteChange> {
    match input.u8()? {
        0 => Some(NoteChange::Write { words: p::bytes(input, sizes.detail)? }),
        1 => Some(NoteChange::Remove),
        _ => None,
    }
}
#[must_use]
pub fn encode_note_change(value: &NoteChange, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_note_change(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_note_change(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_note_change(bytes: &[u8], sizes: &Sizes) -> Option<NoteChange> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_note_change(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_note_change(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Recall {
    Named { name: Box<[u8]> },
    Search { words: Box<[u8]>, most: u32 },
}
pub(crate) fn put_recall(out: &mut Encoder, value: &Recall, sizes: &Sizes) -> Option<()> {
    match value {
        Recall::Named { name } => {
            out.u8(0)?;
            out.bytes(name, sizes.name_bytes)?;
        }
        Recall::Search { words, most } => {
            out.u8(1)?;
            out.bytes(words, sizes.detail)?;
            out.u32(*most)?;
        }
    }
    Some(())
}
pub(crate) fn get_recall(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Recall> {
    match input.u8()? {
        0 => Some(Recall::Named { name: p::bytes(input, sizes.name_bytes)? }),
        1 => Some(Recall::Search { words: p::bytes(input, sizes.detail)?, most: input.u32()? }),
        _ => None,
    }
}
#[must_use]
pub fn encode_recall(value: &Recall, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_recall(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_recall(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_recall(bytes: &[u8], sizes: &Sizes) -> Option<Recall> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_recall(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_recall(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(u64::from(sizes.name_bytes))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ForgeRead {
    Pull { repository: Name, number: u64, head: Option<[u8; 32]> },
    Files { repository: Name, head: [u8; 32] },
    Diff { repository: Name, base: [u8; 32], head: [u8; 32] },
    Ci { repository: Name, head: [u8; 32] },
    Issue { repository: Name, number: u64 },
    Comments { resource: Name, after: Option<u64> },
}
pub(crate) fn put_forge_read(out: &mut Encoder, value: &ForgeRead, sizes: &Sizes) -> Option<()> {
    match value {
        ForgeRead::Pull { repository, number, head } => {
            out.u8(0)?;
            put_name(out, repository, sizes)?;
            out.u64(*number)?;
            match head {
                Some(element) => {
                    out.u8(1)?;
                    out.raw(element)?;
                }
                None => {
                    out.u8(0)?;
                }
            }
        }
        ForgeRead::Files { repository, head } => {
            out.u8(1)?;
            put_name(out, repository, sizes)?;
            out.raw(head)?;
        }
        ForgeRead::Diff { repository, base, head } => {
            out.u8(2)?;
            put_name(out, repository, sizes)?;
            out.raw(base)?;
            out.raw(head)?;
        }
        ForgeRead::Ci { repository, head } => {
            out.u8(3)?;
            put_name(out, repository, sizes)?;
            out.raw(head)?;
        }
        ForgeRead::Issue { repository, number } => {
            out.u8(4)?;
            put_name(out, repository, sizes)?;
            out.u64(*number)?;
        }
        ForgeRead::Comments { resource, after } => {
            out.u8(5)?;
            put_name(out, resource, sizes)?;
            match after {
                Some(element) => {
                    out.u8(1)?;
                    out.u64(*element)?;
                }
                None => {
                    out.u8(0)?;
                }
            }
        }
    }
    Some(())
}
pub(crate) fn get_forge_read(input: &mut Reader<'_>, sizes: &Sizes) -> Option<ForgeRead> {
    match input.u8()? {
        0 => Some(ForgeRead::Pull {
            repository: get_name(input, sizes)?,
            number: input.u64()?,
            head: match input.u8()? {
                0 => None,
                1 => Some(input.bytes(32)?.try_into().ok()?),
                _ => return None,
            },
        }),
        1 => Some(ForgeRead::Files { repository: get_name(input, sizes)?, head: input.bytes(32)?.try_into().ok()? }),
        2 => Some(ForgeRead::Diff {
            repository: get_name(input, sizes)?,
            base: input.bytes(32)?.try_into().ok()?,
            head: input.bytes(32)?.try_into().ok()?,
        }),
        3 => Some(ForgeRead::Ci { repository: get_name(input, sizes)?, head: input.bytes(32)?.try_into().ok()? }),
        4 => Some(ForgeRead::Issue { repository: get_name(input, sizes)?, number: input.u64()? }),
        5 => Some(ForgeRead::Comments {
            resource: get_name(input, sizes)?,
            after: match input.u8()? {
                0 => None,
                1 => Some(input.u64()?),
                _ => return None,
            },
        }),
        _ => None,
    }
}
#[must_use]
pub fn encode_forge_read(value: &ForgeRead, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_forge_read(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_forge_read(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_forge_read(bytes: &[u8], sizes: &Sizes) -> Option<ForgeRead> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_forge_read(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_forge_read(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = heap_name(sizes)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = heap_name(sizes)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = heap_name(sizes)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = heap_name(sizes)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = heap_name(sizes)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = heap_name(sizes)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Read {
    Forge { read: ForgeRead },
}
pub(crate) fn put_read(out: &mut Encoder, value: &Read, sizes: &Sizes) -> Option<()> {
    match value {
        Read::Forge { read } => {
            out.u8(0)?;
            put_forge_read(out, read, sizes)?;
        }
    }
    Some(())
}
pub(crate) fn get_read(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Read> {
    match input.u8()? {
        0 => Some(Read::Forge { read: get_forge_read(input, sizes)? }),
        _ => None,
    }
}
#[must_use]
pub fn encode_read(value: &Read, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_read(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_read(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_read(bytes: &[u8], sizes: &Sizes) -> Option<Read> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_read(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_read(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = heap_forge_read(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ForgeEffect {
    OpenPull { repository: Name, branch: Box<[u8]>, base: Box<[u8]>, head: [u8; 32], title: Box<[u8]>, body: Box<[u8]> },
    Merge { repository: Name, number: u64, expected: [u8; 32] },
    Comment { resource: Name, words: Box<[u8]> },
    CreateIssue { repository: Name, title: Box<[u8]>, body: Box<[u8]> },
    EditIssue { repository: Name, number: u64, title: Option<Box<[u8]>>, body: Option<Box<[u8]>> },
    CloseIssue { repository: Name, number: u64 },
    ClosePull { repository: Name, number: u64 },
    PushBranch { repository: Name, branch: Box<[u8]>, head: [u8; 32], expected: Option<[u8; 32]> },
}
pub(crate) fn put_forge_effect(out: &mut Encoder, value: &ForgeEffect, sizes: &Sizes) -> Option<()> {
    match value {
        ForgeEffect::OpenPull { repository, branch, base, head, title, body } => {
            out.u8(0)?;
            put_name(out, repository, sizes)?;
            out.bytes(branch, sizes.name_bytes)?;
            out.bytes(base, sizes.name_bytes)?;
            out.raw(head)?;
            out.bytes(title, sizes.detail)?;
            out.bytes(body, sizes.detail)?;
        }
        ForgeEffect::Merge { repository, number, expected } => {
            out.u8(1)?;
            put_name(out, repository, sizes)?;
            out.u64(*number)?;
            out.raw(expected)?;
        }
        ForgeEffect::Comment { resource, words } => {
            out.u8(2)?;
            put_name(out, resource, sizes)?;
            out.bytes(words, sizes.detail)?;
        }
        ForgeEffect::CreateIssue { repository, title, body } => {
            out.u8(3)?;
            put_name(out, repository, sizes)?;
            out.bytes(title, sizes.detail)?;
            out.bytes(body, sizes.detail)?;
        }
        ForgeEffect::EditIssue { repository, number, title, body } => {
            out.u8(4)?;
            put_name(out, repository, sizes)?;
            out.u64(*number)?;
            match title {
                Some(element) => {
                    out.u8(1)?;
                    out.bytes(element, sizes.detail)?;
                }
                None => {
                    out.u8(0)?;
                }
            }
            match body {
                Some(element) => {
                    out.u8(1)?;
                    out.bytes(element, sizes.detail)?;
                }
                None => {
                    out.u8(0)?;
                }
            }
        }
        ForgeEffect::CloseIssue { repository, number } => {
            out.u8(5)?;
            put_name(out, repository, sizes)?;
            out.u64(*number)?;
        }
        ForgeEffect::ClosePull { repository, number } => {
            out.u8(6)?;
            put_name(out, repository, sizes)?;
            out.u64(*number)?;
        }
        ForgeEffect::PushBranch { repository, branch, head, expected } => {
            out.u8(7)?;
            put_name(out, repository, sizes)?;
            out.bytes(branch, sizes.name_bytes)?;
            out.raw(head)?;
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
pub(crate) fn get_forge_effect(input: &mut Reader<'_>, sizes: &Sizes) -> Option<ForgeEffect> {
    match input.u8()? {
        0 => Some(ForgeEffect::OpenPull {
            repository: get_name(input, sizes)?,
            branch: p::bytes(input, sizes.name_bytes)?,
            base: p::bytes(input, sizes.name_bytes)?,
            head: input.bytes(32)?.try_into().ok()?,
            title: p::bytes(input, sizes.detail)?,
            body: p::bytes(input, sizes.detail)?,
        }),
        1 => Some(ForgeEffect::Merge {
            repository: get_name(input, sizes)?,
            number: input.u64()?,
            expected: input.bytes(32)?.try_into().ok()?,
        }),
        2 => Some(ForgeEffect::Comment { resource: get_name(input, sizes)?, words: p::bytes(input, sizes.detail)? }),
        3 => Some(ForgeEffect::CreateIssue {
            repository: get_name(input, sizes)?,
            title: p::bytes(input, sizes.detail)?,
            body: p::bytes(input, sizes.detail)?,
        }),
        4 => Some(ForgeEffect::EditIssue {
            repository: get_name(input, sizes)?,
            number: input.u64()?,
            title: match input.u8()? {
                0 => None,
                1 => Some(p::bytes(input, sizes.detail)?),
                _ => return None,
            },
            body: match input.u8()? {
                0 => None,
                1 => Some(p::bytes(input, sizes.detail)?),
                _ => return None,
            },
        }),
        5 => Some(ForgeEffect::CloseIssue { repository: get_name(input, sizes)?, number: input.u64()? }),
        6 => Some(ForgeEffect::ClosePull { repository: get_name(input, sizes)?, number: input.u64()? }),
        7 => Some(ForgeEffect::PushBranch {
            repository: get_name(input, sizes)?,
            branch: p::bytes(input, sizes.name_bytes)?,
            head: input.bytes(32)?.try_into().ok()?,
            expected: match input.u8()? {
                0 => None,
                1 => Some(input.bytes(32)?.try_into().ok()?),
                _ => return None,
            },
        }),
        _ => None,
    }
}
#[must_use]
pub fn encode_forge_effect(value: &ForgeEffect, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_forge_effect(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_forge_effect(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_forge_effect(bytes: &[u8], sizes: &Sizes) -> Option<ForgeEffect> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_forge_effect(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_forge_effect(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = heap_name(sizes)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.name_bytes))?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.name_bytes))?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = heap_name(sizes)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = heap_name(sizes)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = heap_name(sizes)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = heap_name(sizes)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = heap_name(sizes)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = heap_name(sizes)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = heap_name(sizes)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.name_bytes))?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Effect {
    Forge { effect: ForgeEffect },
}
pub(crate) fn put_effect(out: &mut Encoder, value: &Effect, sizes: &Sizes) -> Option<()> {
    match value {
        Effect::Forge { effect } => {
            out.u8(0)?;
            put_forge_effect(out, effect, sizes)?;
        }
    }
    Some(())
}
pub(crate) fn get_effect(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Effect> {
    match input.u8()? {
        0 => Some(Effect::Forge { effect: get_forge_effect(input, sizes)? }),
        _ => None,
    }
}
#[must_use]
pub fn encode_effect(value: &Effect, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_effect(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_effect(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_effect(bytes: &[u8], sizes: &Sizes) -> Option<Effect> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_effect(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_effect(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = heap_forge_effect(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Action {
    Delegate { batch: Box<[NewTask]> },
    Message { to: u64, words: Box<[u8]>, kind: MessageKind },
    Amend { task: u64, amendment: Amendment },
    Cancel { task: u64, reason: Box<[u8]> },
    Release { task: u64 },
    Decide { waiting: u64, decision: Decision },
    Subscribe { topic: Topic },
    Unsubscribe { topic: Topic },
    Effect { connector: u16, effect: Effect },
    Note { scope: u32, name: Box<[u8]>, revision: Option<u64>, change: NoteChange },
    Widen { task: u64, authority: Authority },
}
pub(crate) fn put_action(out: &mut Encoder, value: &Action, sizes: &Sizes) -> Option<()> {
    match value {
        Action::Delegate { batch } => {
            out.u8(0)?;
            let count_21 = u32::try_from(batch.len()).ok()?;
            if count_21 > sizes.entries {
                return None;
            }
            out.u32(count_21)?;
            for element in batch {
                put_new_task(out, element, sizes)?;
            }
        }
        Action::Message { to, words, kind } => {
            out.u8(1)?;
            out.u64(*to)?;
            out.bytes(words, sizes.detail)?;
            put_message_kind(out, kind, sizes)?;
        }
        Action::Amend { task, amendment } => {
            out.u8(2)?;
            out.u64(*task)?;
            put_amendment(out, amendment, sizes)?;
        }
        Action::Cancel { task, reason } => {
            out.u8(3)?;
            out.u64(*task)?;
            out.bytes(reason, sizes.detail)?;
        }
        Action::Release { task } => {
            out.u8(4)?;
            out.u64(*task)?;
        }
        Action::Decide { waiting, decision } => {
            out.u8(5)?;
            out.u64(*waiting)?;
            put_decision(out, decision, sizes)?;
        }
        Action::Subscribe { topic } => {
            out.u8(6)?;
            put_topic(out, topic, sizes)?;
        }
        Action::Unsubscribe { topic } => {
            out.u8(7)?;
            put_topic(out, topic, sizes)?;
        }
        Action::Effect { connector, effect } => {
            out.u8(8)?;
            out.u16(*connector)?;
            put_effect(out, effect, sizes)?;
        }
        Action::Note { scope, name, revision, change } => {
            out.u8(9)?;
            out.u32(*scope)?;
            out.bytes(name, sizes.name_bytes)?;
            match revision {
                Some(element) => {
                    out.u8(1)?;
                    out.u64(*element)?;
                }
                None => {
                    out.u8(0)?;
                }
            }
            put_note_change(out, change, sizes)?;
        }
        Action::Widen { task, authority } => {
            out.u8(10)?;
            out.u64(*task)?;
            put_authority(out, authority, sizes)?;
        }
    }
    Some(())
}
pub(crate) fn get_action(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Action> {
    match input.u8()? {
        0 => Some(Action::Delegate {
            batch: {
                let count = p::count(input, sizes.entries, 84)?;
                let mut values = List::with_capacity(count);
                for _ in 0..count {
                    let value = get_new_task(input, sizes)?;
                    values.push(value).expect("validated array capacity");
                }
                values.into_boxed()
            },
        }),
        1 => Some(Action::Message {
            to: input.u64()?,
            words: p::bytes(input, sizes.detail)?,
            kind: get_message_kind(input, sizes)?,
        }),
        2 => Some(Action::Amend { task: input.u64()?, amendment: get_amendment(input, sizes)? }),
        3 => Some(Action::Cancel { task: input.u64()?, reason: p::bytes(input, sizes.detail)? }),
        4 => Some(Action::Release { task: input.u64()? }),
        5 => Some(Action::Decide { waiting: input.u64()?, decision: get_decision(input, sizes)? }),
        6 => Some(Action::Subscribe { topic: get_topic(input, sizes)? }),
        7 => Some(Action::Unsubscribe { topic: get_topic(input, sizes)? }),
        8 => Some(Action::Effect { connector: input.u16()?, effect: get_effect(input, sizes)? }),
        9 => Some(Action::Note {
            scope: input.u32()?,
            name: p::bytes(input, sizes.name_bytes)?,
            revision: match input.u8()? {
                0 => None,
                1 => Some(input.u64()?),
                _ => return None,
            },
            change: get_note_change(input, sizes)?,
        }),
        10 => Some(Action::Widen { task: input.u64()?, authority: get_authority(input, sizes)? }),
        _ => None,
    }
}
#[must_use]
pub fn encode_action(value: &Action, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_action(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_action(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_action(bytes: &[u8], sizes: &Sizes) -> Option<Action> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_action(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_action(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = u64::try_from(size_of::<NewTask>())
            .ok()?
            .checked_add(heap_new_task(sizes)?)?
            .checked_mul(u64::from(sizes.entries))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        let field = heap_message_kind(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = heap_amendment(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.detail))?;
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
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = heap_decision(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = heap_topic(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = heap_topic(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = heap_effect(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.name_bytes))?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = heap_note_change(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = heap_authority(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Call {
    Delegate { batch: Box<[NewTask]> },
    Message { to: u64, words: Box<[u8]>, kind: MessageKind },
    Amend { task: u64, amendment: Amendment },
    Cancel { task: u64, reason: Box<[u8]> },
    Release { task: u64 },
    Decide { waiting: u64, decision: Decision },
    Propose { action: Action, reason: Box<[u8]>, accepter_requests: bool },
    Subscribe { topic: Topic },
    Unsubscribe { topic: Topic },
    Effect { connector: u16, effect: Effect },
    Note { scope: u32, name: Box<[u8]>, revision: Option<u64>, change: NoteChange },
    Recall { scope: u32, query: Recall },
    Read { connector: u16, read: Read },
}
pub(crate) fn put_call(out: &mut Encoder, value: &Call, sizes: &Sizes) -> Option<()> {
    match value {
        Call::Delegate { batch } => {
            out.u8(0)?;
            let count_22 = u32::try_from(batch.len()).ok()?;
            if count_22 > sizes.entries {
                return None;
            }
            out.u32(count_22)?;
            for element in batch {
                put_new_task(out, element, sizes)?;
            }
        }
        Call::Message { to, words, kind } => {
            out.u8(1)?;
            out.u64(*to)?;
            out.bytes(words, sizes.detail)?;
            put_message_kind(out, kind, sizes)?;
        }
        Call::Amend { task, amendment } => {
            out.u8(2)?;
            out.u64(*task)?;
            put_amendment(out, amendment, sizes)?;
        }
        Call::Cancel { task, reason } => {
            out.u8(3)?;
            out.u64(*task)?;
            out.bytes(reason, sizes.detail)?;
        }
        Call::Release { task } => {
            out.u8(4)?;
            out.u64(*task)?;
        }
        Call::Decide { waiting, decision } => {
            out.u8(5)?;
            out.u64(*waiting)?;
            put_decision(out, decision, sizes)?;
        }
        Call::Propose { action, reason, accepter_requests } => {
            out.u8(6)?;
            put_action(out, action, sizes)?;
            out.bytes(reason, sizes.detail)?;
            out.bool(*accepter_requests)?;
        }
        Call::Subscribe { topic } => {
            out.u8(7)?;
            put_topic(out, topic, sizes)?;
        }
        Call::Unsubscribe { topic } => {
            out.u8(8)?;
            put_topic(out, topic, sizes)?;
        }
        Call::Effect { connector, effect } => {
            out.u8(9)?;
            out.u16(*connector)?;
            put_effect(out, effect, sizes)?;
        }
        Call::Note { scope, name, revision, change } => {
            out.u8(10)?;
            out.u32(*scope)?;
            out.bytes(name, sizes.name_bytes)?;
            match revision {
                Some(element) => {
                    out.u8(1)?;
                    out.u64(*element)?;
                }
                None => {
                    out.u8(0)?;
                }
            }
            put_note_change(out, change, sizes)?;
        }
        Call::Recall { scope, query } => {
            out.u8(11)?;
            out.u32(*scope)?;
            put_recall(out, query, sizes)?;
        }
        Call::Read { connector, read } => {
            out.u8(12)?;
            out.u16(*connector)?;
            put_read(out, read, sizes)?;
        }
    }
    Some(())
}
pub(crate) fn get_call(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Call> {
    match input.u8()? {
        0 => Some(Call::Delegate {
            batch: {
                let count = p::count(input, sizes.entries, 84)?;
                let mut values = List::with_capacity(count);
                for _ in 0..count {
                    let value = get_new_task(input, sizes)?;
                    values.push(value).expect("validated array capacity");
                }
                values.into_boxed()
            },
        }),
        1 => Some(Call::Message {
            to: input.u64()?,
            words: p::bytes(input, sizes.detail)?,
            kind: get_message_kind(input, sizes)?,
        }),
        2 => Some(Call::Amend { task: input.u64()?, amendment: get_amendment(input, sizes)? }),
        3 => Some(Call::Cancel { task: input.u64()?, reason: p::bytes(input, sizes.detail)? }),
        4 => Some(Call::Release { task: input.u64()? }),
        5 => Some(Call::Decide { waiting: input.u64()?, decision: get_decision(input, sizes)? }),
        6 => Some(Call::Propose {
            action: get_action(input, sizes)?,
            reason: p::bytes(input, sizes.detail)?,
            accepter_requests: p::boolean(input)?,
        }),
        7 => Some(Call::Subscribe { topic: get_topic(input, sizes)? }),
        8 => Some(Call::Unsubscribe { topic: get_topic(input, sizes)? }),
        9 => Some(Call::Effect { connector: input.u16()?, effect: get_effect(input, sizes)? }),
        10 => Some(Call::Note {
            scope: input.u32()?,
            name: p::bytes(input, sizes.name_bytes)?,
            revision: match input.u8()? {
                0 => None,
                1 => Some(input.u64()?),
                _ => return None,
            },
            change: get_note_change(input, sizes)?,
        }),
        11 => Some(Call::Recall { scope: input.u32()?, query: get_recall(input, sizes)? }),
        12 => Some(Call::Read { connector: input.u16()?, read: get_read(input, sizes)? }),
        _ => None,
    }
}
#[must_use]
pub fn encode_call(value: &Call, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_call(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_call(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_call(bytes: &[u8], sizes: &Sizes) -> Option<Call> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_call(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
#[expect(clippy::too_many_lines, reason = "one complete bound over the closed tool vocabulary")]
pub fn heap_call(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = u64::try_from(size_of::<NewTask>())
            .ok()?
            .checked_add(heap_new_task(sizes)?)?
            .checked_mul(u64::from(sizes.entries))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        let field = heap_message_kind(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = heap_amendment(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.detail))?;
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
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = heap_decision(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = heap_action(sizes)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = heap_topic(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = heap_topic(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = heap_effect(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.name_bytes))?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = heap_note_change(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = heap_recall(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = heap_read(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Outcome {
    Change { title: Box<[u8]>, body: Box<[u8]> },
    Verdict { verdict: u32, fields: Box<[Field]>, follow_ups: Box<[NewTask]> },
    Report { text: Box<[u8]> },
    Failure { reason: Box<[u8]> },
}
pub(crate) fn put_outcome(out: &mut Encoder, value: &Outcome, sizes: &Sizes) -> Option<()> {
    match value {
        Outcome::Change { title, body } => {
            out.u8(0)?;
            out.bytes(title, sizes.detail)?;
            out.bytes(body, sizes.detail)?;
        }
        Outcome::Verdict { verdict, fields, follow_ups } => {
            out.u8(1)?;
            out.u32(*verdict)?;
            let count_23 = u32::try_from(fields.len()).ok()?;
            if count_23 > sizes.entries {
                return None;
            }
            out.u32(count_23)?;
            for element in fields {
                put_field(out, element, sizes)?;
            }
            let count_24 = u32::try_from(follow_ups.len()).ok()?;
            if count_24 > sizes.entries {
                return None;
            }
            out.u32(count_24)?;
            for element in follow_ups {
                put_new_task(out, element, sizes)?;
            }
        }
        Outcome::Report { text } => {
            out.u8(2)?;
            out.bytes(text, sizes.detail)?;
        }
        Outcome::Failure { reason } => {
            out.u8(3)?;
            out.bytes(reason, sizes.detail)?;
        }
    }
    Some(())
}
pub(crate) fn get_outcome(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Outcome> {
    match input.u8()? {
        0 => Some(Outcome::Change { title: p::bytes(input, sizes.detail)?, body: p::bytes(input, sizes.detail)? }),
        1 => Some(Outcome::Verdict {
            verdict: input.u32()?,
            fields: {
                let count = p::count(input, sizes.entries, 8)?;
                let mut values = List::with_capacity(count);
                for _ in 0..count {
                    let value = get_field(input, sizes)?;
                    values.push(value).expect("validated array capacity");
                }
                values.into_boxed()
            },
            follow_ups: {
                let count = p::count(input, sizes.entries, 84)?;
                let mut values = List::with_capacity(count);
                for _ in 0..count {
                    let value = get_new_task(input, sizes)?;
                    values.push(value).expect("validated array capacity");
                }
                values.into_boxed()
            },
        }),
        2 => Some(Outcome::Report { text: p::bytes(input, sizes.detail)? }),
        3 => Some(Outcome::Failure { reason: p::bytes(input, sizes.detail)? }),
        _ => None,
    }
}
#[must_use]
pub fn encode_outcome(value: &Outcome, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_outcome(&mut out, value, sizes)?;
    if out.length() > sizes.outcome {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_outcome(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_outcome(bytes: &[u8], sizes: &Sizes) -> Option<Outcome> {
    if u32::try_from(bytes.len()).ok()? > sizes.outcome {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_outcome(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_outcome(sizes: &Sizes) -> Option<u64> {
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
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = u64::try_from(size_of::<Field>())
            .ok()?
            .checked_add(heap_field(sizes)?)?
            .checked_mul(u64::from(sizes.entries))?;
        total = total.checked_add(field)?;
        let field = u64::try_from(size_of::<NewTask>())
            .ok()?
            .checked_add(heap_new_task(sizes)?)?
            .checked_mul(u64::from(sizes.entries))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Ending {
    Done,
    Failed,
    Cancelled,
}
pub(crate) fn put_ending(out: &mut Encoder, value: &Ending, _sizes: &Sizes) -> Option<()> {
    match value {
        Ending::Done => {
            out.u8(0)?;
        }
        Ending::Failed => {
            out.u8(1)?;
        }
        Ending::Cancelled => {
            out.u8(2)?;
        }
    }
    Some(())
}
pub(crate) fn get_ending(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<Ending> {
    match input.u8()? {
        0 => Some(Ending::Done),
        1 => Some(Ending::Failed),
        2 => Some(Ending::Cancelled),
        _ => None,
    }
}
#[must_use]
pub fn encode_ending(value: &Ending, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_ending(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_ending(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_ending(bytes: &[u8], sizes: &Sizes) -> Option<Ending> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_ending(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_ending(_sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Party {
    Task { task: u64 },
    Person { person: u64 },
    Deployment,
}
pub(crate) fn put_party(out: &mut Encoder, value: &Party, _sizes: &Sizes) -> Option<()> {
    match value {
        Party::Task { task } => {
            out.u8(0)?;
            out.u64(*task)?;
        }
        Party::Person { person } => {
            out.u8(1)?;
            out.u64(*person)?;
        }
        Party::Deployment => {
            out.u8(2)?;
        }
    }
    Some(())
}
pub(crate) fn get_party(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<Party> {
    match input.u8()? {
        0 => Some(Party::Task { task: input.u64()? }),
        1 => Some(Party::Person { person: input.u64()? }),
        2 => Some(Party::Deployment),
        _ => None,
    }
}
#[must_use]
pub fn encode_party(value: &Party, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_party(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_party(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_party(bytes: &[u8], sizes: &Sizes) -> Option<Party> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_party(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_party(_sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
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
        let total = 0_u64;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Class {
    Critical,
    Ordinary,
}
pub(crate) fn put_class(out: &mut Encoder, value: &Class, _sizes: &Sizes) -> Option<()> {
    match value {
        Class::Critical => {
            out.u8(0)?;
        }
        Class::Ordinary => {
            out.u8(1)?;
        }
    }
    Some(())
}
pub(crate) fn get_class(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<Class> {
    match input.u8()? {
        0 => Some(Class::Critical),
        1 => Some(Class::Ordinary),
        _ => None,
    }
}
#[must_use]
pub fn encode_class(value: &Class, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_class(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_class(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_class(bytes: &[u8], sizes: &Sizes) -> Option<Class> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_class(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_class(_sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ForgeNews {
    Pull { repository: Name, number: u64, head: [u8; 32] },
    Ci { repository: Name, head: [u8; 32], passed: bool },
    Issue { repository: Name, number: u64 },
    Comment { resource: Name, number: u64 },
}
pub(crate) fn put_forge_news(out: &mut Encoder, value: &ForgeNews, sizes: &Sizes) -> Option<()> {
    match value {
        ForgeNews::Pull { repository, number, head } => {
            out.u8(0)?;
            put_name(out, repository, sizes)?;
            out.u64(*number)?;
            out.raw(head)?;
        }
        ForgeNews::Ci { repository, head, passed } => {
            out.u8(1)?;
            put_name(out, repository, sizes)?;
            out.raw(head)?;
            out.bool(*passed)?;
        }
        ForgeNews::Issue { repository, number } => {
            out.u8(2)?;
            put_name(out, repository, sizes)?;
            out.u64(*number)?;
        }
        ForgeNews::Comment { resource, number } => {
            out.u8(3)?;
            put_name(out, resource, sizes)?;
            out.u64(*number)?;
        }
    }
    Some(())
}
pub(crate) fn get_forge_news(input: &mut Reader<'_>, sizes: &Sizes) -> Option<ForgeNews> {
    match input.u8()? {
        0 => Some(ForgeNews::Pull {
            repository: get_name(input, sizes)?,
            number: input.u64()?,
            head: input.bytes(32)?.try_into().ok()?,
        }),
        1 => Some(ForgeNews::Ci {
            repository: get_name(input, sizes)?,
            head: input.bytes(32)?.try_into().ok()?,
            passed: p::boolean(input)?,
        }),
        2 => Some(ForgeNews::Issue { repository: get_name(input, sizes)?, number: input.u64()? }),
        3 => Some(ForgeNews::Comment { resource: get_name(input, sizes)?, number: input.u64()? }),
        _ => None,
    }
}
#[must_use]
pub fn encode_forge_news(value: &ForgeNews, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_forge_news(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_forge_news(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_forge_news(bytes: &[u8], sizes: &Sizes) -> Option<ForgeNews> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_forge_news(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_forge_news(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = heap_name(sizes)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = heap_name(sizes)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = heap_name(sizes)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = heap_name(sizes)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum News {
    Forge { news: ForgeNews },
}
pub(crate) fn put_news(out: &mut Encoder, value: &News, sizes: &Sizes) -> Option<()> {
    match value {
        News::Forge { news } => {
            out.u8(0)?;
            put_forge_news(out, news, sizes)?;
        }
    }
    Some(())
}
pub(crate) fn get_news(input: &mut Reader<'_>, sizes: &Sizes) -> Option<News> {
    match input.u8()? {
        0 => Some(News::Forge { news: get_forge_news(input, sizes)? }),
        _ => None,
    }
}
#[must_use]
pub fn encode_news(value: &News, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_news(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_news(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_news(bytes: &[u8], sizes: &Sizes) -> Option<News> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_news(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_news(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = heap_forge_news(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Notice {
    Held { reason: Box<[u8]> },
    Released,
    Cancelled { reason: Box<[u8]> },
    EffectFailed { effect: u64, reason: Box<[u8]> },
    MessageLost { message: u64 },
    Budget { remaining: u64 },
}
pub(crate) fn put_notice(out: &mut Encoder, value: &Notice, sizes: &Sizes) -> Option<()> {
    match value {
        Notice::Held { reason } => {
            out.u8(0)?;
            out.bytes(reason, sizes.detail)?;
        }
        Notice::Released => {
            out.u8(1)?;
        }
        Notice::Cancelled { reason } => {
            out.u8(2)?;
            out.bytes(reason, sizes.detail)?;
        }
        Notice::EffectFailed { effect, reason } => {
            out.u8(3)?;
            out.u64(*effect)?;
            out.bytes(reason, sizes.detail)?;
        }
        Notice::MessageLost { message } => {
            out.u8(4)?;
            out.u64(*message)?;
        }
        Notice::Budget { remaining } => {
            out.u8(5)?;
            out.u64(*remaining)?;
        }
    }
    Some(())
}
pub(crate) fn get_notice(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Notice> {
    match input.u8()? {
        0 => Some(Notice::Held { reason: p::bytes(input, sizes.detail)? }),
        1 => Some(Notice::Released),
        2 => Some(Notice::Cancelled { reason: p::bytes(input, sizes.detail)? }),
        3 => Some(Notice::EffectFailed { effect: input.u64()?, reason: p::bytes(input, sizes.detail)? }),
        4 => Some(Notice::MessageLost { message: input.u64()? }),
        5 => Some(Notice::Budget { remaining: input.u64()? }),
        _ => None,
    }
}
#[must_use]
pub fn encode_notice(value: &Notice, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_notice(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_notice(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_notice(bytes: &[u8], sizes: &Sizes) -> Option<Notice> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_notice(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_notice(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.detail))?;
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
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Message {
    Result { from: u64, ending: Ending, result: Outcome },
    Question { from: u64, words: Box<[u8]> },
    Answer { from: u64, words: Box<[u8]> },
    Decision { proposal: u64, decision: Decision },
    Amendment { amendment: Amendment },
    Words { from: Party, words: Box<[u8]> },
    News { topic: Topic, class: Class, news: News },
    Notice { notice: Notice },
    Timer { at: u64 },
    Waiting { proposal: u64, action: Action, reason: Box<[u8]> },
}
pub(crate) fn put_message(out: &mut Encoder, value: &Message, sizes: &Sizes) -> Option<()> {
    match value {
        Message::Result { from, ending, result } => {
            out.u8(0)?;
            out.u64(*from)?;
            put_ending(out, ending, sizes)?;
            put_outcome(out, result, sizes)?;
        }
        Message::Question { from, words } => {
            out.u8(1)?;
            out.u64(*from)?;
            out.bytes(words, sizes.detail)?;
        }
        Message::Answer { from, words } => {
            out.u8(2)?;
            out.u64(*from)?;
            out.bytes(words, sizes.detail)?;
        }
        Message::Decision { proposal, decision } => {
            out.u8(3)?;
            out.u64(*proposal)?;
            put_decision(out, decision, sizes)?;
        }
        Message::Amendment { amendment } => {
            out.u8(4)?;
            put_amendment(out, amendment, sizes)?;
        }
        Message::Words { from, words } => {
            out.u8(5)?;
            put_party(out, from, sizes)?;
            out.bytes(words, sizes.detail)?;
        }
        Message::News { topic, class, news } => {
            out.u8(6)?;
            put_topic(out, topic, sizes)?;
            put_class(out, class, sizes)?;
            put_news(out, news, sizes)?;
        }
        Message::Notice { notice } => {
            out.u8(7)?;
            put_notice(out, notice, sizes)?;
        }
        Message::Timer { at } => {
            out.u8(8)?;
            out.u64(*at)?;
        }
        Message::Waiting { proposal, action, reason } => {
            out.u8(9)?;
            out.u64(*proposal)?;
            put_action(out, action, sizes)?;
            out.bytes(reason, sizes.detail)?;
        }
    }
    Some(())
}
pub(crate) fn get_message(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Message> {
    match input.u8()? {
        0 => Some(Message::Result {
            from: input.u64()?,
            ending: get_ending(input, sizes)?,
            result: get_outcome(input, sizes)?,
        }),
        1 => Some(Message::Question { from: input.u64()?, words: p::bytes(input, sizes.detail)? }),
        2 => Some(Message::Answer { from: input.u64()?, words: p::bytes(input, sizes.detail)? }),
        3 => Some(Message::Decision { proposal: input.u64()?, decision: get_decision(input, sizes)? }),
        4 => Some(Message::Amendment { amendment: get_amendment(input, sizes)? }),
        5 => Some(Message::Words { from: get_party(input, sizes)?, words: p::bytes(input, sizes.detail)? }),
        6 => Some(Message::News {
            topic: get_topic(input, sizes)?,
            class: get_class(input, sizes)?,
            news: get_news(input, sizes)?,
        }),
        7 => Some(Message::Notice { notice: get_notice(input, sizes)? }),
        8 => Some(Message::Timer { at: input.u64()? }),
        9 => Some(Message::Waiting {
            proposal: input.u64()?,
            action: get_action(input, sizes)?,
            reason: p::bytes(input, sizes.detail)?,
        }),
        _ => None,
    }
}
#[must_use]
pub fn encode_message(value: &Message, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_message(&mut out, value, sizes)?;
    if out.length() > sizes.inbound {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_message(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_message(bytes: &[u8], sizes: &Sizes) -> Option<Message> {
    if u32::try_from(bytes.len()).ok()? > sizes.inbound {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_message(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_message(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = heap_ending(sizes)?;
        total = total.checked_add(field)?;
        let field = heap_outcome(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = heap_decision(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = heap_amendment(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = heap_party(sizes)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = heap_topic(sizes)?;
        total = total.checked_add(field)?;
        let field = heap_class(sizes)?;
        total = total.checked_add(field)?;
        let field = heap_news(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = heap_notice(sizes)?;
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
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = heap_action(sizes)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ForgeAnswer {
    Pull { number: u64, head: [u8; 32], title: Box<[u8]>, body: Box<[u8]>, open: bool },
    Files { files: Box<[File]>, more: bool },
    Diff { patch: Box<[u8]>, cut: u64 },
    Ci { head: [u8; 32], statuses: Box<[CiStatus]>, more: bool },
    Issue { number: u64, title: Box<[u8]>, body: Box<[u8]>, open: bool },
    Comments { comments: Box<[Comment]>, more: bool },
}
pub(crate) fn put_forge_answer(out: &mut Encoder, value: &ForgeAnswer, sizes: &Sizes) -> Option<()> {
    match value {
        ForgeAnswer::Pull { number, head, title, body, open } => {
            out.u8(0)?;
            out.u64(*number)?;
            out.raw(head)?;
            out.bytes(title, sizes.detail)?;
            out.bytes(body, sizes.detail)?;
            out.bool(*open)?;
        }
        ForgeAnswer::Files { files, more } => {
            out.u8(1)?;
            let count_25 = u32::try_from(files.len()).ok()?;
            if count_25 > sizes.entries {
                return None;
            }
            out.u32(count_25)?;
            for element in files {
                put_file(out, element, sizes)?;
            }
            out.bool(*more)?;
        }
        ForgeAnswer::Diff { patch, cut } => {
            out.u8(2)?;
            out.bytes(patch, sizes.detail)?;
            out.u64(*cut)?;
        }
        ForgeAnswer::Ci { head, statuses, more } => {
            out.u8(3)?;
            out.raw(head)?;
            let count_26 = u32::try_from(statuses.len()).ok()?;
            if count_26 > sizes.entries {
                return None;
            }
            out.u32(count_26)?;
            for element in statuses {
                put_ci_status(out, element, sizes)?;
            }
            out.bool(*more)?;
        }
        ForgeAnswer::Issue { number, title, body, open } => {
            out.u8(4)?;
            out.u64(*number)?;
            out.bytes(title, sizes.detail)?;
            out.bytes(body, sizes.detail)?;
            out.bool(*open)?;
        }
        ForgeAnswer::Comments { comments, more } => {
            out.u8(5)?;
            let count_27 = u32::try_from(comments.len()).ok()?;
            if count_27 > sizes.entries {
                return None;
            }
            out.u32(count_27)?;
            for element in comments {
                put_comment(out, element, sizes)?;
            }
            out.bool(*more)?;
        }
    }
    Some(())
}
pub(crate) fn get_forge_answer(input: &mut Reader<'_>, sizes: &Sizes) -> Option<ForgeAnswer> {
    match input.u8()? {
        0 => Some(ForgeAnswer::Pull {
            number: input.u64()?,
            head: input.bytes(32)?.try_into().ok()?,
            title: p::bytes(input, sizes.detail)?,
            body: p::bytes(input, sizes.detail)?,
            open: p::boolean(input)?,
        }),
        1 => Some(ForgeAnswer::Files {
            files: {
                let count = p::count(input, sizes.entries, 8)?;
                let mut values = List::with_capacity(count);
                for _ in 0..count {
                    let value = get_file(input, sizes)?;
                    values.push(value).expect("validated array capacity");
                }
                values.into_boxed()
            },
            more: p::boolean(input)?,
        }),
        2 => Some(ForgeAnswer::Diff { patch: p::bytes(input, sizes.detail)?, cut: input.u64()? }),
        3 => Some(ForgeAnswer::Ci {
            head: input.bytes(32)?.try_into().ok()?,
            statuses: {
                let count = p::count(input, sizes.entries, 6)?;
                let mut values = List::with_capacity(count);
                for _ in 0..count {
                    let value = get_ci_status(input, sizes)?;
                    values.push(value).expect("validated array capacity");
                }
                values.into_boxed()
            },
            more: p::boolean(input)?,
        }),
        4 => Some(ForgeAnswer::Issue {
            number: input.u64()?,
            title: p::bytes(input, sizes.detail)?,
            body: p::bytes(input, sizes.detail)?,
            open: p::boolean(input)?,
        }),
        5 => Some(ForgeAnswer::Comments {
            comments: {
                let count = p::count(input, sizes.entries, 12)?;
                let mut values = List::with_capacity(count);
                for _ in 0..count {
                    let value = get_comment(input, sizes)?;
                    values.push(value).expect("validated array capacity");
                }
                values.into_boxed()
            },
            more: p::boolean(input)?,
        }),
        _ => None,
    }
}
#[must_use]
pub fn encode_forge_answer(value: &ForgeAnswer, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_forge_answer(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_forge_answer(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_forge_answer(bytes: &[u8], sizes: &Sizes) -> Option<ForgeAnswer> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_forge_answer(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_forge_answer(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = u64::try_from(size_of::<File>())
            .ok()?
            .checked_add(heap_file(sizes)?)?
            .checked_mul(u64::from(sizes.entries))?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = u64::try_from(size_of::<CiStatus>())
            .ok()?
            .checked_add(heap_ci_status(sizes)?)?
            .checked_mul(u64::from(sizes.entries))?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = u64::try_from(size_of::<Comment>())
            .ok()?
            .checked_add(heap_comment(sizes)?)?
            .checked_mul(u64::from(sizes.entries))?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ReadAnswer {
    Forge { answer: ForgeAnswer },
}
pub(crate) fn put_read_answer(out: &mut Encoder, value: &ReadAnswer, sizes: &Sizes) -> Option<()> {
    match value {
        ReadAnswer::Forge { answer } => {
            out.u8(0)?;
            put_forge_answer(out, answer, sizes)?;
        }
    }
    Some(())
}
pub(crate) fn get_read_answer(input: &mut Reader<'_>, sizes: &Sizes) -> Option<ReadAnswer> {
    match input.u8()? {
        0 => Some(ReadAnswer::Forge { answer: get_forge_answer(input, sizes)? }),
        _ => None,
    }
}
#[must_use]
pub fn encode_read_answer(value: &ReadAnswer, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_read_answer(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_read_answer(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_read_answer(bytes: &[u8], sizes: &Sizes) -> Option<ReadAnswer> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_read_answer(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_read_answer(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = heap_forge_answer(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum EffectResult {
    Made { resource: Name, revision: Option<[u8; 32]> },
    Pending { effect: u64 },
    Failed { reason: Box<[u8]> },
}
pub(crate) fn put_effect_result(out: &mut Encoder, value: &EffectResult, sizes: &Sizes) -> Option<()> {
    match value {
        EffectResult::Made { resource, revision } => {
            out.u8(0)?;
            put_name(out, resource, sizes)?;
            match revision {
                Some(element) => {
                    out.u8(1)?;
                    out.raw(element)?;
                }
                None => {
                    out.u8(0)?;
                }
            }
        }
        EffectResult::Pending { effect } => {
            out.u8(1)?;
            out.u64(*effect)?;
        }
        EffectResult::Failed { reason } => {
            out.u8(2)?;
            out.bytes(reason, sizes.detail)?;
        }
    }
    Some(())
}
pub(crate) fn get_effect_result(input: &mut Reader<'_>, sizes: &Sizes) -> Option<EffectResult> {
    match input.u8()? {
        0 => Some(EffectResult::Made {
            resource: get_name(input, sizes)?,
            revision: match input.u8()? {
                0 => None,
                1 => Some(input.bytes(32)?.try_into().ok()?),
                _ => return None,
            },
        }),
        1 => Some(EffectResult::Pending { effect: input.u64()? }),
        2 => Some(EffectResult::Failed { reason: p::bytes(input, sizes.detail)? }),
        _ => None,
    }
}
#[must_use]
pub fn encode_effect_result(value: &EffectResult, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_effect_result(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_effect_result(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_effect_result(bytes: &[u8], sizes: &Sizes) -> Option<EffectResult> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_effect_result(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_effect_result(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = heap_name(sizes)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
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
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Lack {
    Tool { family: u8 },
    Grant { connector: u16, kind: u16, resource: Name },
    Delegation { kind: ExecutorKind },
    Spend { missing: u64 },
    Depth,
    Tasks,
    Notes { scope: u8 },
    Deadline,
}
pub(crate) fn put_lack(out: &mut Encoder, value: &Lack, sizes: &Sizes) -> Option<()> {
    match value {
        Lack::Tool { family } => {
            out.u8(0)?;
            out.u8(*family)?;
        }
        Lack::Grant { connector, kind, resource } => {
            out.u8(1)?;
            out.u16(*connector)?;
            out.u16(*kind)?;
            put_name(out, resource, sizes)?;
        }
        Lack::Delegation { kind } => {
            out.u8(2)?;
            put_executor_kind(out, kind, sizes)?;
        }
        Lack::Spend { missing } => {
            out.u8(3)?;
            out.u64(*missing)?;
        }
        Lack::Depth => {
            out.u8(4)?;
        }
        Lack::Tasks => {
            out.u8(5)?;
        }
        Lack::Notes { scope } => {
            out.u8(6)?;
            out.u8(*scope)?;
        }
        Lack::Deadline => {
            out.u8(7)?;
        }
    }
    Some(())
}
pub(crate) fn get_lack(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Lack> {
    match input.u8()? {
        0 => Some(Lack::Tool { family: input.u8()? }),
        1 => Some(Lack::Grant { connector: input.u16()?, kind: input.u16()?, resource: get_name(input, sizes)? }),
        2 => Some(Lack::Delegation { kind: get_executor_kind(input, sizes)? }),
        3 => Some(Lack::Spend { missing: input.u64()? }),
        4 => Some(Lack::Depth),
        5 => Some(Lack::Tasks),
        6 => Some(Lack::Notes { scope: input.u8()? }),
        7 => Some(Lack::Deadline),
        _ => None,
    }
}
#[must_use]
pub fn encode_lack(value: &Lack, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_lack(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_lack(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_lack(bytes: &[u8], sizes: &Sizes) -> Option<Lack> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_lack(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_lack(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = heap_name(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = heap_executor_kind(sizes)?;
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
        let total = 0_u64;
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
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Unserved {
    Lost,
    Withdrawn,
    Busy,
    Unavailable,
    TooLarge,
}
pub(crate) fn put_unserved(out: &mut Encoder, value: &Unserved, _sizes: &Sizes) -> Option<()> {
    match value {
        Unserved::Lost => {
            out.u8(0)?;
        }
        Unserved::Withdrawn => {
            out.u8(1)?;
        }
        Unserved::Busy => {
            out.u8(2)?;
        }
        Unserved::Unavailable => {
            out.u8(3)?;
        }
        Unserved::TooLarge => {
            out.u8(4)?;
        }
    }
    Some(())
}
pub(crate) fn get_unserved(input: &mut Reader<'_>, _sizes: &Sizes) -> Option<Unserved> {
    match input.u8()? {
        0 => Some(Unserved::Lost),
        1 => Some(Unserved::Withdrawn),
        2 => Some(Unserved::Busy),
        3 => Some(Unserved::Unavailable),
        4 => Some(Unserved::TooLarge),
        _ => None,
    }
}
#[must_use]
pub fn encode_unserved(value: &Unserved, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_unserved(&mut out, value, sizes)?;
    if out.length() > sizes.call {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_unserved(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_unserved(bytes: &[u8], sizes: &Sizes) -> Option<Unserved> {
    if u32::try_from(bytes.len()).ok()? > sizes.call {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_unserved(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_unserved(_sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    {
        let total = 0_u64;
        largest = largest.max(total);
    }
    Some(largest)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Served {
    Delegated { tasks: Box<[u64]> },
    Sent { message: u64 },
    Done,
    Proposed { proposal: u64, holder: Party },
    Decision { accepted: bool, passed_to: Option<Party> },
    Effect { result: EffectResult },
    Notes { notes: Box<[Note]>, more: bool },
    Read { answer: ReadAnswer },
    Refused { task: Option<u32>, reason: Box<[u8]> },
    Beyond { lacked: Box<[Lack]> },
    Unserved { reason: Unserved },
}
pub(crate) fn put_served(out: &mut Encoder, value: &Served, sizes: &Sizes) -> Option<()> {
    match value {
        Served::Delegated { tasks } => {
            out.u8(0)?;
            let count_28 = u32::try_from(tasks.len()).ok()?;
            if count_28 > sizes.entries {
                return None;
            }
            out.u32(count_28)?;
            for element in tasks {
                out.u64(*element)?;
            }
        }
        Served::Sent { message } => {
            out.u8(1)?;
            out.u64(*message)?;
        }
        Served::Done => {
            out.u8(2)?;
        }
        Served::Proposed { proposal, holder } => {
            out.u8(3)?;
            out.u64(*proposal)?;
            put_party(out, holder, sizes)?;
        }
        Served::Decision { accepted, passed_to } => {
            out.u8(4)?;
            out.bool(*accepted)?;
            match passed_to {
                Some(element) => {
                    out.u8(1)?;
                    put_party(out, element, sizes)?;
                }
                None => {
                    out.u8(0)?;
                }
            }
        }
        Served::Effect { result } => {
            out.u8(5)?;
            put_effect_result(out, result, sizes)?;
        }
        Served::Notes { notes, more } => {
            out.u8(6)?;
            let count_29 = u32::try_from(notes.len()).ok()?;
            if count_29 > sizes.entries {
                return None;
            }
            out.u32(count_29)?;
            for element in notes {
                put_note(out, element, sizes)?;
            }
            out.bool(*more)?;
        }
        Served::Read { answer } => {
            out.u8(7)?;
            put_read_answer(out, answer, sizes)?;
        }
        Served::Refused { task, reason } => {
            out.u8(8)?;
            match task {
                Some(element) => {
                    out.u8(1)?;
                    out.u32(*element)?;
                }
                None => {
                    out.u8(0)?;
                }
            }
            out.bytes(reason, sizes.detail)?;
        }
        Served::Beyond { lacked } => {
            out.u8(9)?;
            let count_30 = u32::try_from(lacked.len()).ok()?;
            if count_30 > sizes.entries {
                return None;
            }
            out.u32(count_30)?;
            for element in lacked {
                put_lack(out, element, sizes)?;
            }
        }
        Served::Unserved { reason } => {
            out.u8(10)?;
            put_unserved(out, reason, sizes)?;
        }
    }
    Some(())
}
pub(crate) fn get_served(input: &mut Reader<'_>, sizes: &Sizes) -> Option<Served> {
    match input.u8()? {
        0 => Some(Served::Delegated {
            tasks: {
                let count = p::count(input, sizes.entries, 8)?;
                let mut values = List::with_capacity(count);
                for _ in 0..count {
                    let value = input.u64()?;
                    values.push(value).expect("validated array capacity");
                }
                values.into_boxed()
            },
        }),
        1 => Some(Served::Sent { message: input.u64()? }),
        2 => Some(Served::Done),
        3 => Some(Served::Proposed { proposal: input.u64()?, holder: get_party(input, sizes)? }),
        4 => Some(Served::Decision {
            accepted: p::boolean(input)?,
            passed_to: match input.u8()? {
                0 => None,
                1 => Some(get_party(input, sizes)?),
                _ => return None,
            },
        }),
        5 => Some(Served::Effect { result: get_effect_result(input, sizes)? }),
        6 => Some(Served::Notes {
            notes: {
                let count = p::count(input, sizes.entries, 16)?;
                let mut values = List::with_capacity(count);
                for _ in 0..count {
                    let value = get_note(input, sizes)?;
                    values.push(value).expect("validated array capacity");
                }
                values.into_boxed()
            },
            more: p::boolean(input)?,
        }),
        7 => Some(Served::Read { answer: get_read_answer(input, sizes)? }),
        8 => Some(Served::Refused {
            task: match input.u8()? {
                0 => None,
                1 => Some(input.u32()?),
                _ => return None,
            },
            reason: p::bytes(input, sizes.detail)?,
        }),
        9 => Some(Served::Beyond {
            lacked: {
                let count = p::count(input, sizes.entries, 1)?;
                let mut values = List::with_capacity(count);
                for _ in 0..count {
                    let value = get_lack(input, sizes)?;
                    values.push(value).expect("validated array capacity");
                }
                values.into_boxed()
            },
        }),
        10 => Some(Served::Unserved { reason: get_unserved(input, sizes)? }),
        _ => None,
    }
}
#[must_use]
pub fn encode_served(value: &Served, sizes: &Sizes) -> Option<Box<[u8]>> {
    let mut out = Encoder::measure();
    put_served(&mut out, value, sizes)?;
    if out.length() > sizes.answer {
        return None;
    }
    let mut out = Encoder::writing(out.length());
    put_served(&mut out, value, sizes)?;
    Some(out.finish())
}
#[must_use]
pub fn decode_served(bytes: &[u8], sizes: &Sizes) -> Option<Served> {
    if u32::try_from(bytes.len()).ok()? > sizes.answer {
        return None;
    }
    let mut input = Reader::new(bytes);
    let value = get_served(&mut input, sizes)?;
    if input.is_empty() { Some(value) } else { None }
}
#[must_use]
pub fn heap_served(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    {
        let mut total = 0_u64;
        let field =
            u64::try_from(size_of::<u64>()).ok()?.checked_add(Some(0_u64)?)?.checked_mul(u64::from(sizes.entries))?;
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
        let total = 0_u64;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = heap_party(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = heap_party(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = heap_effect_result(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = u64::try_from(size_of::<Note>())
            .ok()?
            .checked_add(heap_note(sizes)?)?
            .checked_mul(u64::from(sizes.entries))?;
        total = total.checked_add(field)?;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = heap_read_answer(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = Some(0_u64)?;
        total = total.checked_add(field)?;
        let field = Some(u64::from(sizes.detail))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = u64::try_from(size_of::<Lack>())
            .ok()?
            .checked_add(heap_lack(sizes)?)?
            .checked_mul(u64::from(sizes.entries))?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    {
        let mut total = 0_u64;
        let field = heap_unserved(sizes)?;
        total = total.checked_add(field)?;
        largest = largest.max(total);
    }
    Some(largest)
}

// Encoded-size amplification prevents independent nested count limits
// from multiplying a bound past what the enclosing payload can contain.
fn amplification_budget() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    Some(largest)
}
fn amplification_prices() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    Some(largest)
}
fn amplification_model() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_model_kind());
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_prices()?);
    Some(largest)
}
fn amplification_section() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(amplification_section_kind());
    largest = largest.max(Some(1_u64)?);
    Some(largest)
}
fn amplification_field_rule() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(0_u64)?);
    Some(largest)
}
fn amplification_verdict_rule() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(
        u64::try_from(size_of::<FieldRule>())
            .ok()?
            .checked_add(4_u64)?
            .checked_div(5_u64)?
            .checked_add(amplification_field_rule()?)?,
    );
    largest = largest
        .max(u64::try_from(size_of::<u32>()).ok()?.checked_add(3_u64)?.checked_div(4_u64)?.checked_add(Some(0_u64)?)?);
    largest = largest.max(Some(0_u64)?);
    Some(largest)
}
fn amplification_charter() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(
        u64::try_from(size_of::<Section>())
            .ok()?
            .checked_add(4_u64)?
            .checked_div(5_u64)?
            .checked_add(amplification_section()?)?,
    );
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_contract()?);
    largest = largest.max(amplification_budget()?);
    largest = largest.max(
        u64::try_from(size_of::<Model>())
            .ok()?
            .checked_add(40_u64)?
            .checked_div(41_u64)?
            .checked_add(amplification_model()?)?,
    );
    largest = largest.max(Some(0_u64)?);
    Some(largest)
}
fn amplification_name() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(
        u64::try_from(size_of::<Box<[u8]>>())
            .ok()?
            .checked_add(3_u64)?
            .checked_div(4_u64)?
            .checked_add(Some(1_u64)?)?,
    );
    Some(largest)
}
fn amplification_pattern() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(
        u64::try_from(size_of::<Box<[u8]>>())
            .ok()?
            .checked_add(3_u64)?
            .checked_div(4_u64)?
            .checked_add(Some(1_u64)?)?,
    );
    largest = largest.max(amplification_last()?);
    Some(largest)
}
fn amplification_grant() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_pattern()?);
    Some(largest)
}
fn amplification_delegation() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(
        u64::try_from(size_of::<ExecutorKind>())
            .ok()?
            .checked_add(4_u64)?
            .checked_div(5_u64)?
            .checked_add(amplification_executor_kind()?)?,
    );
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    Some(largest)
}
fn amplification_authority() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(
        u64::try_from(size_of::<Grant>())
            .ok()?
            .checked_add(8_u64)?
            .checked_div(9_u64)?
            .checked_add(amplification_grant()?)?,
    );
    largest = largest.max(amplification_delegation()?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    Some(largest)
}
fn amplification_resources() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(
        u64::try_from(size_of::<Resource>())
            .ok()?
            .checked_add(4_u64)?
            .checked_div(5_u64)?
            .checked_add(amplification_resource()?)?,
    );
    largest = largest.max(
        u64::try_from(size_of::<Resource>())
            .ok()?
            .checked_add(4_u64)?
            .checked_div(5_u64)?
            .checked_add(amplification_resource()?)?,
    );
    Some(largest)
}
fn amplification_wake() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(amplification_wake_class());
    largest = largest.max(amplification_wake_class());
    largest = largest.max(amplification_wake_class());
    largest = largest.max(amplification_wake_class());
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    Some(largest)
}
fn amplification_spec() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(amplification_resources()?);
    largest = largest
        .max(u64::try_from(size_of::<u64>()).ok()?.checked_add(7_u64)?.checked_div(8_u64)?.checked_add(Some(0_u64)?)?);
    Some(largest)
}
fn amplification_new_task() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(amplification_spec()?);
    largest = largest.max(amplification_task_contract()?);
    largest = largest.max(amplification_executor()?);
    largest = largest.max(amplification_authority()?);
    largest = largest.max(
        u64::try_from(size_of::<Dependency>())
            .ok()?
            .checked_add(4_u64)?
            .checked_div(5_u64)?
            .checked_add(amplification_dependency()?)?,
    );
    largest = largest
        .max(u64::try_from(size_of::<u64>()).ok()?.checked_add(7_u64)?.checked_div(8_u64)?.checked_add(Some(0_u64)?)?);
    largest = largest.max(amplification_wake()?);
    largest = largest.max(
        u64::try_from(size_of::<Topic>())
            .ok()?
            .checked_add(6_u64)?
            .checked_div(7_u64)?
            .checked_add(amplification_topic()?)?,
    );
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    Some(largest)
}
fn amplification_amendment() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(amplification_spec()?);
    largest = largest.max(amplification_wake()?);
    largest = largest
        .max(u64::try_from(size_of::<u64>()).ok()?.checked_add(7_u64)?.checked_div(8_u64)?.checked_add(Some(0_u64)?)?);
    largest = largest.max(amplification_authority()?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(amplification_procedure_parameters()?);
    Some(largest)
}
fn amplification_note() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(1_u64)?);
    Some(largest)
}
fn amplification_field() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(1_u64)?);
    Some(largest)
}
fn amplification_inbound() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_message()?);
    Some(largest)
}
fn amplification_file() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(1_u64)?);
    Some(largest)
}
fn amplification_ci_status() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    Some(largest)
}
fn amplification_comment() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(1_u64)?);
    Some(largest)
}
fn amplification_turn() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    Some(largest)
}
fn amplification_committed_call() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_call()?);
    largest = largest.max(amplification_served()?);
    Some(largest)
}
fn amplification_transcript() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(
        u64::try_from(size_of::<Turn>())
            .ok()?
            .checked_add(14_u64)?
            .checked_div(15_u64)?
            .checked_add(amplification_turn()?)?,
    );
    largest = largest.max(
        u64::try_from(size_of::<CommittedCall>())
            .ok()?
            .checked_add(13_u64)?
            .checked_div(14_u64)?
            .checked_add(amplification_committed_call()?)?,
    );
    Some(largest)
}
fn amplification_model_kind() -> u64 {
    0
}
fn amplification_section_kind() -> u64 {
    0
}
fn amplification_contract() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(
        u64::try_from(size_of::<VerdictRule>())
            .ok()?
            .checked_add(19_u64)?
            .checked_div(20_u64)?
            .checked_add(amplification_verdict_rule()?)?,
    );
    largest = largest.max(Some(0_u64)?);
    Some(largest)
}
fn amplification_last() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(1_u64)?);
    Some(largest)
}
fn amplification_executor_kind() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    Some(largest)
}
fn amplification_branch_role() -> u64 {
    0
}
fn amplification_resource() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(amplification_name()?);
    largest = largest.max(amplification_name()?);
    largest = largest.max(amplification_branch_role());
    Some(largest)
}
fn amplification_dependency() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    Some(largest)
}
fn amplification_wake_class() -> u64 {
    0
}
fn amplification_topic() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_name()?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    Some(largest)
}
fn amplification_procedure_parameters() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(amplification_name()?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_topic()?);
    Some(largest)
}
fn amplification_executor() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_procedure_parameters()?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(
        u64::try_from(size_of::<Box<[u8]>>())
            .ok()?
            .checked_add(3_u64)?
            .checked_div(4_u64)?
            .checked_add(Some(1_u64)?)?,
    );
    Some(largest)
}
fn amplification_task_contract() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(amplification_contract()?);
    largest = largest.max(
        u64::try_from(size_of::<Box<[u8]>>())
            .ok()?
            .checked_add(3_u64)?
            .checked_div(4_u64)?
            .checked_add(Some(1_u64)?)?,
    );
    largest = largest.max(Some(0_u64)?);
    Some(largest)
}
fn amplification_decision() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(Some(1_u64)?);
    Some(largest)
}
fn amplification_message_kind() -> u64 {
    0
}
fn amplification_note_change() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(Some(1_u64)?);
    Some(largest)
}
fn amplification_recall() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(0_u64)?);
    Some(largest)
}
fn amplification_forge_read() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(amplification_name()?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_name()?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_name()?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_name()?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_name()?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_name()?);
    largest = largest.max(Some(0_u64)?);
    Some(largest)
}
fn amplification_read() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(amplification_forge_read()?);
    Some(largest)
}
fn amplification_forge_effect() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(amplification_name()?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(amplification_name()?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_name()?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(amplification_name()?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(amplification_name()?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(amplification_name()?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_name()?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_name()?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    Some(largest)
}
fn amplification_effect() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(amplification_forge_effect()?);
    Some(largest)
}
fn amplification_action() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(
        u64::try_from(size_of::<NewTask>())
            .ok()?
            .checked_add(83_u64)?
            .checked_div(84_u64)?
            .checked_add(amplification_new_task()?)?,
    );
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(amplification_message_kind());
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_amendment()?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_decision()?);
    largest = largest.max(amplification_topic()?);
    largest = largest.max(amplification_topic()?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_effect()?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_note_change()?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_authority()?);
    Some(largest)
}
fn amplification_call() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(
        u64::try_from(size_of::<NewTask>())
            .ok()?
            .checked_add(83_u64)?
            .checked_div(84_u64)?
            .checked_add(amplification_new_task()?)?,
    );
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(amplification_message_kind());
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_amendment()?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_decision()?);
    largest = largest.max(amplification_action()?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_topic()?);
    largest = largest.max(amplification_topic()?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_effect()?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_note_change()?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_recall()?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_read()?);
    Some(largest)
}
fn amplification_outcome() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(
        u64::try_from(size_of::<Field>())
            .ok()?
            .checked_add(7_u64)?
            .checked_div(8_u64)?
            .checked_add(amplification_field()?)?,
    );
    largest = largest.max(
        u64::try_from(size_of::<NewTask>())
            .ok()?
            .checked_add(83_u64)?
            .checked_div(84_u64)?
            .checked_add(amplification_new_task()?)?,
    );
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(1_u64)?);
    Some(largest)
}
fn amplification_ending() -> u64 {
    0
}
fn amplification_party() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    Some(largest)
}
fn amplification_class() -> u64 {
    0
}
fn amplification_forge_news() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(amplification_name()?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_name()?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_name()?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_name()?);
    largest = largest.max(Some(0_u64)?);
    Some(largest)
}
fn amplification_news() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(amplification_forge_news()?);
    Some(largest)
}
fn amplification_notice() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    Some(largest)
}
fn amplification_message() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_ending());
    largest = largest.max(amplification_outcome()?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_decision()?);
    largest = largest.max(amplification_amendment()?);
    largest = largest.max(amplification_party()?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(amplification_topic()?);
    largest = largest.max(amplification_class());
    largest = largest.max(amplification_news()?);
    largest = largest.max(amplification_notice()?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_action()?);
    largest = largest.max(Some(1_u64)?);
    Some(largest)
}
fn amplification_forge_answer() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(
        u64::try_from(size_of::<File>())
            .ok()?
            .checked_add(7_u64)?
            .checked_div(8_u64)?
            .checked_add(amplification_file()?)?,
    );
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(
        u64::try_from(size_of::<CiStatus>())
            .ok()?
            .checked_add(5_u64)?
            .checked_div(6_u64)?
            .checked_add(amplification_ci_status()?)?,
    );
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(
        u64::try_from(size_of::<Comment>())
            .ok()?
            .checked_add(11_u64)?
            .checked_div(12_u64)?
            .checked_add(amplification_comment()?)?,
    );
    largest = largest.max(Some(0_u64)?);
    Some(largest)
}
fn amplification_read_answer() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(amplification_forge_answer()?);
    Some(largest)
}
fn amplification_effect_result() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(amplification_name()?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(1_u64)?);
    Some(largest)
}
fn amplification_lack() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_name()?);
    largest = largest.max(amplification_executor_kind()?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    Some(largest)
}
fn amplification_unserved() -> u64 {
    0
}
fn amplification_served() -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest
        .max(u64::try_from(size_of::<u64>()).ok()?.checked_add(7_u64)?.checked_div(8_u64)?.checked_add(Some(0_u64)?)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_party()?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_party()?);
    largest = largest.max(amplification_effect_result()?);
    largest = largest.max(
        u64::try_from(size_of::<Note>())
            .ok()?
            .checked_add(15_u64)?
            .checked_div(16_u64)?
            .checked_add(amplification_note()?)?,
    );
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(amplification_read_answer()?);
    largest = largest.max(Some(0_u64)?);
    largest = largest.max(Some(1_u64)?);
    largest = largest.max(
        u64::try_from(size_of::<Lack>())
            .ok()?
            .checked_add(0_u64)?
            .checked_div(1_u64)?
            .checked_add(amplification_lack()?)?,
    );
    largest = largest.max(amplification_unserved());
    Some(largest)
}
/// Decoded heap for an exact bounded charter payload, excluding its owned input.
#[must_use]
pub fn decoded_heap_charter(sizes: &Sizes) -> Option<u64> {
    Some(heap_charter(sizes)?.min(u64::from(sizes.charter).checked_mul(amplification_charter()?)?))
}
/// Decoded heap for an exact bounded inbound payload, excluding its owned input.
#[must_use]
pub fn decoded_heap_inbound(sizes: &Sizes) -> Option<u64> {
    Some(heap_inbound(sizes)?.min(u64::from(sizes.inbound).checked_mul(amplification_inbound()?)?))
}
/// Decoded heap for an exact bounded call payload, excluding its owned input.
#[must_use]
pub fn decoded_heap_call(sizes: &Sizes) -> Option<u64> {
    Some(heap_call(sizes)?.min(u64::from(sizes.call).checked_mul(amplification_call()?)?))
}
/// Decoded heap for an exact bounded served payload, excluding its owned input.
#[must_use]
pub fn decoded_heap_served(sizes: &Sizes) -> Option<u64> {
    Some(heap_served(sizes)?.min(u64::from(sizes.answer).checked_mul(amplification_served()?)?))
}
/// Decoded heap for an exact bounded outcome payload, excluding its owned input.
#[must_use]
pub fn decoded_heap_outcome(sizes: &Sizes) -> Option<u64> {
    Some(heap_outcome(sizes)?.min(u64::from(sizes.outcome).checked_mul(amplification_outcome()?)?))
}
/// Decoded heap for an exact bounded turn payload, excluding its owned input.
#[must_use]
pub fn decoded_heap_turn(sizes: &Sizes) -> Option<u64> {
    Some(heap_turn(sizes)?.min(u64::from(sizes.turn).checked_mul(amplification_turn()?)?))
}
/// Decoded heap for an exact bounded transcript payload, excluding its owned input.
#[must_use]
pub fn decoded_heap_transcript(sizes: &Sizes) -> Option<u64> {
    Some(heap_transcript(sizes)?.min(u64::from(sizes.transcript).checked_mul(amplification_transcript()?)?))
}
/// One encoded input and its decoded owned value, with no unbounded nesting.
#[must_use]
pub fn worst_case(sizes: &Sizes) -> Option<u64> {
    let mut largest = 0_u64;
    largest = largest.max(u64::from(sizes.charter).checked_add(decoded_heap_charter(sizes)?)?);
    largest = largest.max(u64::from(sizes.inbound).checked_add(decoded_heap_inbound(sizes)?)?);
    largest = largest.max(u64::from(sizes.call).checked_add(decoded_heap_call(sizes)?)?);
    largest = largest.max(u64::from(sizes.answer).checked_add(decoded_heap_served(sizes)?)?);
    largest = largest.max(u64::from(sizes.outcome).checked_add(decoded_heap_outcome(sizes)?)?);
    largest = largest.max(u64::from(sizes.turn).checked_add(decoded_heap_turn(sizes)?)?);
    largest = largest.max(u64::from(sizes.transcript).checked_add(decoded_heap_transcript(sizes)?)?);
    Some(largest)
}
