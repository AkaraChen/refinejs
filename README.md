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
comparisons, equality, boolean values, `!`, `&&`, and `||`. Numeric operations
are solved as IEEE-754 binary64 values, matching JavaScript `Number` rounding,
NaN, and infinity behavior; division is deliberately outside the checked
subset. Function parameter and return refinements are checked both inside the
function and at every call site.

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
cargo run -- check fixtures/sqrt.js
cargo run -- build fixtures/sqrt.js output.js
```

## Static subset

- `/*#rt */` comments attach to functions, parameters, and variables.
- Number and boolean expressions, calls to refined functions, `let`/`const`
  declarations and assignments, blocks, `if`, and early `return` are checked.
- Unsupported expressions or statements in checked code produce a static
  error instead of being silently accepted.
- Default/rest parameters, async/generator functions, `var`, destructuring,
  compound assignment, and division are rejected rather than approximated.
- Runtime instrumentation uses source-fresh temporary identifiers; `__rt` is a
  reserved binding because emitted assertions call `__rt.assert`.
- The prelude provides contracts for `Math.sqrt`, `Math.abs`,
  `Array.isArray`, and `Number.isInteger`.

## Built with

- Rust
- [oxc](https://oxc.rs/) for JS parsing
- [Z3](https://github.com/Z3Prover/z3) for fixedpoint/Horn solving

## License

MIT
