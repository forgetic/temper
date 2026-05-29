# Lesson 0005: Avoid redundant pending in queue labels

## Tags

`workflow`, `naming`, `labels`

## Trigger

A post-merge owner review label was proposed as `alignment-pending`, and the
human clarified that queue-oriented labels already imply pending work.

## What went wrong

The suggested name repeated the queue semantics in the label itself, making the
label longer and more mechanical than the workflow vocabulary needed.

## Steering for future agents

When naming labels that exist only to route work into a queue, prefer the concise
work concept (`alignment`) over adding `pending`, `awaiting`, or similar suffixes
unless the spec needs to distinguish pending from completed states explicitly.

## Where this is now documented

The reference delivery fixture uses the post-merge `alignment` label for the
owner's holistic review queue in
`crates/harness-workflow/fixtures/reference-delivery.json`.
