# refinejs

Refinement type annotations in JavaScript comments.

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

## CLI

```bash
cargo run -- check fixtures/sqrt.js
cargo run -- build fixtures/sqrt.js output.js
```

## Day 1 scope

- Parser extracts `#rt` comments and attaches them to functions, parameters, and variables.
- Checker validates that refinement predicates reference known identifiers.
- Transpiler injects runtime `__rt.assert(...)` calls for parameters, return values, and variables.
- Minimal prelude for `Math.sqrt`, `Math.abs`, `Array.isArray`, `Number.isInteger`.

## Built with

- Rust
- [oxc](https://oxc.rs/) for JS parsing

## License

MIT
