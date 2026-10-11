# 03 — lessons, conformance model (append-only; one agent per file)

## The verdict I did not expect to be able to make

The brief asked me to choose between schema-parse and capnp introspection and
justify it. I expected to have to argue from principle. I got to argue from *running
it*, which is better: `capnp compile -o -` dumps the CodeGeneratorRequest, and
`capnp convert binary:text` decodes it with capnp's own `schema.capnp`. I ran both on
one schema and on all 89. The mechanism exists, it is installed, and it is the same
artifact `build.rs` already feeds to `capnpc`.

Lesson: **when a brief gives you two mechanisms to choose between, run both before you
write the argument.** The one that works is the argument.

## The thing I learned that reframes the whole model

`capnp 0.27.2` is already a pinned dependency, and it ships `schema_capnp` — the
generated Rust for the compiler's own schema — plus `schema::StructSchema`,
`Field::get_type()`, `EnumSchema`, and `introspect::TypeVariant`. So the conformance
generator needs **no new dependency, no new tool, and no schema parser at all**. It
reads the same typed structures `build.rs` consumes.

That means "generated from the contract" is not a promise about a script. It is a
statement about which artifact you read. The code under test is generated from the
CGR; read the CGR or you are testing a surface the crate does not have.

## The honest-flag finding

I set out to count `*Valid` fields and found the contract had already solved the
classification problem for me, in the naming:

- 9 `Bool` fields end in capital-**`Valid`** → honesty flags ("was this measured?").
- 12 `Bool` fields end in lowercase `valid`/`invalid` → verdicts ("is this signature
  valid?").

A verdict being false is normal. An honesty flag being false is the *only* legal way
for a measurement to be absent. An audit that conflates them demands a measurement
where the contract asks for a judgement — and a backend then has to fabricate to pass.
The distinction is 100% consistent across 89 schemas. Seven of the nine resolve their
partner by name; two do not, honestly: `cli.CommandPresentation.noArgsValid` is a
*documentation* question (correctly excluded), and
`display.Monitor.refreshRateRangeValid` qualifies `refreshRateMin`/`refreshRateMax`
without being name-derivable from either. I reported the limitation rather than
papering over it.

Lesson: **look for the classification the contract already made before inventing one.**
The discrimination is usually already there, encoded in convention.

## The E8 connection I did not expect

267 E8 findings mean 267 enums whose `@0` is a paradigm value — `ColorSpace.srgb`,
`UsbTransferType.control`, `UsbDataRole.host`. A backend that answers an unknown enum
value by defaulting to `@0` does not report ignorance; it fabricates a *specific,
plausible* answer. So the schema's E8 residual is also a fabrication hazard, and the
forward-compat template has to close it: `capnp::NotInSchema(pub u16)` exists and
generated readers return `Result<Which, NotInSchema>`, so the honest assertion is
typed and available. `srgb` is not a measurement.

## Measurement numbers I had to earn, not assert

I initially reported 308 interfaces and 221 DataHandle methods. Both were wrong, and
both were wrong for instructive reasons. The interface count was wrong because the
CodeGeneratorRequest contains the **transitive import closure** — my reader was
counting `KernelService` as belonging to `boot.capnp`. Filtering to nodes whose
display name starts with the domain's own file fixed it: 303 interfaces, which then
matched `rg -c "^interface "` exactly across all 89 files with zero mismatches. The
DataHandle count was wrong because my type comparison used qualified names against
short names.

Lesson: **a generator that counts must be cross-checked against an independent
measurement before its numbers go in a document.** Two of my numbers were wrong when
I first produced them and looked completely plausible. `README.md:65` says 303
interfaces; my first measurement said 308 and the document nearly said so.

## On `capnp convert binary:json`

It crashes: `capnp/compat/json.c++:363: failed: don't know how to JSON-encode
AnyPointer`. The `defaultValue` of every field is an AnyPointer the JSON writer cannot
encode. Not a blocker — the text format decodes fine and is deterministic, and an
in-process Rust generator needs no decoder at all. But worth knowing before someone
writes a JSON pipeline step.

## What I would tell the next agent

- The brief's checklist item on which cases each method shape auto-generates is the
  heart of the deliverable. I answered it with a compiler-signal → case rule table and
  the tree-wide counts for each row, so nobody has to take the taxonomy on faith.
- I built a working proof-of-concept generator (in `/tmp`, not in the repo — the brief
  said create no other files). If the conformance agent wants it, it is 178 lines and
  it is the whole §2.
- One file ownership gap is real and unresolved: `knowledge/availability/**` is where
  the matrix lives and is in nobody's row. I did not create it. Two agents creating it
  independently is exactly the conflict the charter's table exists to prevent.
- `build.rs` consumes the CodeGeneratorRequest and throws it away. Exposing it (or a
  typed inventory derived from it) in `OUT_DIR` is the single highest-leverage change
  the skeleton agent could make for this model, because it makes codegen and
  conformance read one artifact instead of two.
