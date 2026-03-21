// Test that emit-js generated JavaScript is valid and executable.
// Run: node examples/test-js-runtime.mjs

let passed = 0;
let failed = 0;

function assert(name, actual, expected) {
  if (JSON.stringify(actual) === JSON.stringify(expected)) {
    console.log(`  ✓ ${name}`);
    passed++;
  } else {
    console.log(`  ✗ ${name}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
    failed++;
  }
}

// === Marlang Client Runtime ===
const marlang = (() => {
  const atoms = new Map();
  let atomId = 0;

  function run(gen, handlers) {
    return new Promise((resolve, reject) => {
      function step(value) {
        try {
          const result = gen.next(value);
          if (result.done) { resolve(result.value); return; }
          const eff = result.value;
          if (!eff || !eff.effect) {
            reject(new Error('Non-effect yield: ' + JSON.stringify(eff)));
            return;
          }
          if (handlers && handlers[eff.effect]) {
            handlers[eff.effect](eff.args, step);
          } else {
            reject(new Error('Unhandled effect: ' + eff.effect));
          }
        } catch (e) { reject(e); }
      }
      step(undefined);
    });
  }

  const atomHandlers = {
    atom_new(args, resume) { const id = ++atomId; atoms.set(id, args[0]); resume(id); },
    atom_read(args, resume) { resume(atoms.get(args[0])); },
    atom_swap(args, resume) {
      const ref = args[0], fn = args[1];
      const nv = fn(atoms.get(ref));
      atoms.set(ref, nv);
      resume(nv);
    },
    atom_set(args, resume) { atoms.set(args[0], args[1]); resume(args[1]); }
  };

  return { run, atomHandlers };
})();

// === Tests ===

async function runTests() {
  console.log("1. Literals and operators");
  assert("integer", 42, 42);
  assert("string", "hello", "hello");
  assert("boolean", true, true);
  assert("addition", (1 + 2), 3);
  assert("compound", ((10 + 1) * (20 - 2)), 198);

  console.log("\n2. Lambda");
  assert("arrow fn", ((x) => (x + 1))(5), 6);
  assert("zero-arg", (() => 42)(), 42);

  console.log("\n3. Let (IIFE)");
  assert("let", (() => { const x = 10; const y = 20; return (x + y); })(), 30);

  console.log("\n4. If (ternary)");
  assert("if true", ((5 > 0) ? "pos" : "neg"), "pos");
  assert("if false", ((-1 > 0) ? "pos" : "neg"), "neg");

  console.log("\n5. ADT constructors");
  assert("adt fields", {tag:"Foo",fields:[1,2,3]}, {tag:"Foo",fields:[1,2,3]});
  assert("adt nullary", {tag:"Loading",fields:[]}, {tag:"Loading",fields:[]});
  assert("adt Ok", {tag:"Ok",fields:["hello"]}, {tag:"Ok",fields:["hello"]});

  console.log("\n6. Match");
  const matchTest = (val) => (() => {
    const __m = val;
    if (__m && __m.tag === "Ok") { const v = __m.fields[0]; return "ok:" + v; }
    if (__m && __m.tag === "Err") { const msg = __m.fields[0]; return "err:" + msg; }
    throw new Error('no match');
  })();
  assert("match Ok", matchTest({tag:"Ok",fields:["yes"]}), "ok:yes");
  assert("match Err", matchTest({tag:"Err",fields:["oops"]}), "err:oops");

  console.log("\n7. Atom effects (generator + runtime)");
  // Simple atom: create and read
  const simpleAtom = (function*(initial) {
    const count = (yield {effect:"atom_new",args:[initial]});
    return (yield {effect:"atom_read",args:[count]});
  });
  assert("atom new+read", await marlang.run(simpleAtom(42), marlang.atomHandlers), 42);

  // Atom swap
  const swapTest = (function*(initial) {
    const ref = (yield {effect:"atom_new",args:[initial]});
    (yield {effect:"atom_swap",args:[ref, ((n) => (n + 1))]});
    (yield {effect:"atom_swap",args:[ref, ((n) => (n * 3))]});
    return (yield {effect:"atom_read",args:[ref]});
  });
  assert("atom swap", await marlang.run(swapTest(10), marlang.atomHandlers), 33);

  console.log("\n8. Generator body flattening (let inside lambda*)");
  // Nested let with yields — this is what emit-js generates
  const nestedLet = (function*(x) {
    const a = (yield {effect:"atom_new",args:[x]});
    const b = (yield {effect:"atom_read",args:[a]});
    return (b + 1);
  });
  assert("nested let", await marlang.run(nestedLet(99), marlang.atomHandlers), 100);

  console.log("\n9. Generator body: begin flattening");
  const logs = [];
  const beginTest = (function*() {
    (yield {effect:"print",args:["hello"]});
    (yield {effect:"print",args:["world"]});
    return 42;
  });
  const r9 = await marlang.run(beginTest(), {
    print(args, resume) { logs.push(args[0]); resume(null); }
  });
  assert("begin return", r9, 42);
  assert("begin effects", logs, ["hello", "world"]);

  console.log("\n10. Generator body: if with let");
  const ifLetTest = (function*(x) {
    if ((x > 0)) {
      const a = (yield {effect:"atom_new",args:[x]});
      return (yield {effect:"atom_read",args:[a]});
    } else {
      return 0;
    }
  });
  assert("if+let true", await marlang.run(ifLetTest(42), marlang.atomHandlers), 42);
  assert("if+let false", await marlang.run(ifLetTest(-1), marlang.atomHandlers), 0);

  console.log("\n11. Generator body: match");
  const matchGenTest = (function*(opt) {
    const __m = opt;
    if (__m && __m.tag === "Some") {
      const v = __m.fields[0];
      return (yield {effect:"atom_new",args:[v]});
    }
    if (__m === null || (__m && __m.tag === "None")) {
      return 0;
    }
    throw new Error('no match');
  });
  assert("match Some", await marlang.run(matchGenTest({tag:"Some",fields:[77]}), marlang.atomHandlers) > 0, true);
  assert("match None", await marlang.run(matchGenTest(null), marlang.atomHandlers), 0);

  console.log("\n12. yield* delegation");
  const inner = (function*(x) {
    const ref = (yield {effect:"atom_new",args:[x]});
    return (yield {effect:"atom_read",args:[ref]});
  });
  const outer = (function*(a, b) {
    const x = (yield* inner(a));
    const y = (yield* inner(b));
    return (x + y);
  });
  assert("yield* delegation", await marlang.run(outer(10, 20), marlang.atomHandlers), 30);

  console.log("\n13. handle* with pass-through");
  const innerGen = (function*(x) {
    (yield {effect:"log",args:["before"]});
    const ref = (yield {effect:"atom_new",args:[x]});
    (yield {effect:"log",args:["after"]});
    return (yield {effect:"atom_read",args:[ref]});
  });
  const handleLogs = [];
  // This is what handle* generates — handles log locally, passes atom through
  const handleGenTest = (function*() {
    return (yield* (function*() {
      const __gen = innerGen(99);
      let __r = __gen.next();
      while (!__r.done) {
        const __e = __r.value;
        let __v;
        if (__e.effect === "log") {
          const msg = __e.args[0];
          __v = handleLogs.push(msg);
        } else {
          __v = yield __e;
        }
        __r = __gen.next(__v);
      }
      return __r.value;
    })());
  });
  const r13 = await marlang.run(handleGenTest(), marlang.atomHandlers);
  assert("handle* value", r13, 99);
  assert("handle* logs", handleLogs, ["before", "after"]);

  console.log("\n14. Complex component");
  const compLogs = [];
  const complexComp = (function*(mode, initial) {
    const ref = (yield {effect:"atom_new",args:[initial]});
    const label = ((mode === 1) ? "counter" : "gauge");
    (yield {effect:"print",args:[label]});
    return (label + (": " + String((yield {effect:"atom_read",args:[ref]}))));
  });
  const r14 = await marlang.run(complexComp(1, 42), {
    ...marlang.atomHandlers,
    print(args, resume) { compLogs.push(args[0]); resume(null); }
  });
  assert("complex value", r14, "counter: 42");
  assert("complex print", compLogs, ["counter"]);

  // Summary
  console.log(`\n${'='.repeat(40)}`);
  console.log(`Results: ${passed} passed, ${failed} failed`);
  if (failed > 0) process.exit(1);
}

runTests().catch(e => {
  console.error("FATAL:", e.message);
  console.error(e.stack);
  process.exit(1);
});
