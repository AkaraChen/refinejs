# refinejs

Flux-style refinement types for JavaScript. `refinejs check` statically proves
liquid-type obligations with Z3; `refinejs build` additionally preserves the
existing `__rt.assert` runtime checks in the emitted JavaScript.

## Syntax

Use `/*#rt ... */` comments (deliberately incompatible with JSDoc's `/**`):

```js
/*#rt
 * type: (n: number | n > 0) => number | $ > 0
 */
function sqrt(n) {
  return Math.sqrt(n);
}

/*#rt type: number | x > 0 */
const x = 9;
```

`$` refers to the return value.

Predicate expressions support safe-integer literals, `+`, `-`, `*`, ordered
comparisons, equality, boolean values, `!`, `&&`, and `||`. Ordinary JS
`Number` arithmetic is solved as IEEE-754 binary64. Type indices, dense-array
lengths, and integer loop counters use a separate logical `int` sort; see
[Indexed types and liquid inference](docs/indexed-types-and-liquid-inference.md).
Division is deliberately outside the checked subset. Function parameter and
return refinements are checked both inside the function and at every call site.

Indexed types follow Flux: `number[10]` is the singleton `10`, and
`boolean[0 < n]` is the boolean whose value is the index formula. Names that
appear in an index are logical integers and must be safe integers at call
sites. Array literals introduce an opaque `DenseArray<T>[n]`; `push`/`pop`
update `n`, and `xs[i]` is allowed when `0 <= i < n` is proved. Ordinary
sparse `Array` indexing stays rejected. The flux-rs surface ports, the
rules used to accept or skip a Rust test, and the checker tradeoffs they
forced are in [flux-rs porting rules](docs/flux-rs-porting.md). Browse the
same `fixtures/flux_*.js` cases in the [playground](https://refinejs.vercel.app)
(precomputed `refinejs check` snapshots; Z3 does not run in the browser).

`while` and C-style `for` are checked. Loop-head invariants are inferred by
Houdini over scraped qualifiers (`0 <= v`, `v < length`, postcondition atoms).
A Z3 `unknown` result is a failure, not a proof.

## Path-sensitive checking

`if` conditions become branch-local assumptions. A branch that returns early
is removed from the continuation, so guards narrow the remaining path:

```js
/*#rt type: (x: number) => number | $ > 0 */
function positive(x) {
  if (x <= 0) return 1;
  return x;
}
```

Each subtyping obligation becomes a Horn rule
`environment && !required refinement => bad`. The checker registers `bad` as a
Z3 fixedpoint relation and queries whether it is reachable. An unreachable
`bad` relation is the proof that the subtype obligation holds.

## Refinement polymorphism

Predicate parameters preserve a caller's refinement without naming it in the
generic function:

```js
/*#rt type: forall p. (x: number | p(x)) => number | p($) */
function preserve(x) {
  return x;
}
```

The checker infers `p` from the argument's refinement and instantiates it in
the return type. Predicate parameters are compile-time abstractions, so their
runtime assertion condition is erased to `true`; all concrete `__rt.assert`
checks remain in the output. During generic function checking, applications of
`p` are arbitrary Boolean atoms with congruence for equal arguments; they are
not treated as an empty fixedpoint relation.

## Typestate

Typestate is expressed as an ordinary pre/post refinement. Reassigning the
returned value advances the variable's statically known state:

```js
/*#rt type: (open: boolean | open === true) => boolean | $ === false */
function close(open) {
  return false;
}

/*#rt type: boolean | door === true */
let door = true;
door = close(door);
```

This maps directly to JavaScript value semantics: the state token is a real
integer or boolean value, and transitions happen by returning and assigning a
new token rather than by assuming hidden object mutation.

## CLI

```bash
cargo run -- check --target auto fixtures/sqrt.js
cargo run -- build --target ecmascript fixtures/sqrt.js output.js
```

`--target` selects the refinement-aware standard prelude. Accepted values are
`auto`, `ecmascript`, `browser`, `node`, `deno`, and `bun`. `auto` uses
syntax-aware imports and unbound runtime globals; select a target explicitly
when a source file has no unique runtime marker or deliberately mixes
compatibility APIs. The common ECMAScript catalog includes Array refinements,
while the platform catalogs add DOM/Web APIs, Node modules and globals, Deno
plus its Node compatibility surface, or Bun plus its Node compatibility
surface.

## Compiler-backed library types

The curated prelude describes refinements and mutation/callback effects for
APIs where those semantics matter. To type-check the rest of the APIs visible
to a real TypeScript project—including unmodeled Array methods, DOM members,
and Node/Deno/Bun declarations—run refinejs with an external
[Corsa](https://github.com/ubugeeei-prod/corsa-bind) executable and the exact
project config:

```bash
cargo run -- check \
  --target browser \
  --corsa /absolute/path/to/corsa \
  --tsconfig /absolute/path/to/tsconfig.json \
  /absolute/path/to/source.js
```

Both compiler flags are required. This crate pins the Corsa Rust binding to
`1.12.4` but does not bundle the compiler executable; use a compatible Corsa
build supplied by the caller. The source passed to refinejs must be the exact
on-disk file included by the config.

The `tsconfig` is authoritative for which declarations exist. For JavaScript,
enable `allowJs`, `checkJs`, and strict checking. Choose `lib` entries such as
`ES2025`, `DOM`, and `DOM.Iterable` for browser code, and make the appropriate
Node, Deno, or Bun declarations resolvable through `types`, `typeRoots`, or the
project file set. `--target` independently chooses refinejs's refinement/effect
overlay; it does not add declarations to Corsa.

Compiler errors from every file resolved by the project config gate refinement
checking. To prevent suppressed compiler errors from becoming unsound fallback
evidence, compiler-backed mode asks Corsa for the complete program file set and
rejects TypeScript diagnostic-suppression directives in every implementation
source, as well as configs which disable the required semantic/strict-null
checks. Declaration files remain an explicit compiler trust root. Local
callables need a `/*#rt */` contract; local member implementations cannot be
used as compiler fallback evidence. Compiler-only call and member evidence is
accepted only when Corsa resolves every symbol declaration to a declaration
file; values containing local implementations, or mutable values that crossed
an unknown execution boundary, cannot borrow that declaration's trust.
Compiler-rendered project types do not acquire curated standard-library
refinements merely because their printed names match catalog types. Calls and
getters known only to the compiler are treated as effectful: heap facts,
refinements of mutable bindings, and identities invalidated by reassignment are
forgotten across that boundary.

The complete implementation record, including the trust model, provenance
rules, design choices, verification evidence, and known limits, is in
[Compiler-backed platform refinements](docs/compiler-backed-platform-refinements.md).

## Static subset

- `/*#rt */` comments attach to functions, parameters, and variables.
- Number and boolean expressions, calls to refined functions, `let`/`const`
  declarations and assignments, blocks, `if`, `while`, C-style `for`, and
  early `return` are checked.
- Unsupported expressions or statements in checked code produce a static
  error instead of being silently accepted.
- Default/rest parameters, async/generator functions, `var`, destructuring,
  and division are rejected rather than approximated. `+=` / `-=` and `++` /
  `--` are accepted on tracked numeric bindings.
- Runtime instrumentation uses source-fresh temporary identifiers; `__rt` is a
  reserved binding because emitted assertions call `__rt.assert`.
- Target-specific prelude contracts cover the common ECMAScript collection
  operations and selected browser, Node, Deno, and Bun APIs. Compiler-backed
  mode fills ordinary type information outside that curated semantic overlay.

## Playground

The static playground under `playground/` lists every `fixtures/flux_*.js`
file with its source and a snapshot of `refinejs check --target ecmascript`.
Regenerate the snapshot after checker or fixture changes:

```bash
cargo build
node playground/generate.mjs
```

## Built with

- Rust
- [oxc](https://oxc.rs/) for JS parsing
- [Z3](https://github.com/Z3Prover/z3) for fixedpoint/Horn solving

## License

MIT
