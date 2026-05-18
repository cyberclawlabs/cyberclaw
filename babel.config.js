/**
 * Babel config for the CyberClaw admin SPA.
 *
 * Compiles `web/src/*.jsx` → `web/dist/*.js`.
 *
 * Each file is wrapped in an IIFE so top-level `const`/`let` declarations
 * stay file-local — multiple source files share names like `useState` (12
 * files do `const { useState } = React`), `TRUST_TONE`, `RISK_TONE`. In a
 * plain `<script>` they would collide in global scope.
 *
 * Components / helpers defined in one file are referenced bare from another
 * (e.g., `pages_a.jsx` references `TweaksPanel` defined in `ui.jsx`). To
 * keep that working without modifying source, the IIFE explicitly assigns
 * every top-level binding to `window` at the end of the IIFE — promoting
 * them back to "globals" the way the original `<script type="text/babel">`
 * setup implicitly did.
 *
 * Names destructured from `React` (`useState`, `useEffect`, ...) are also
 * assigned to `window`; harmless because the React UMD doesn't claim those
 * names on `window` itself.
 */
const wrapInIife = ({ types: t }) => ({
  name: 'wrap-program-in-iife-and-expose-globals',
  visitor: {
    Program: {
      exit(path) {
        if (path.node.body.length === 0) return;
        // Idempotency: if already a single IIFE, skip.
        const first = path.node.body[0];
        if (
          path.node.body.length === 1 &&
          first.type === 'ExpressionStatement' &&
          first.expression.type === 'CallExpression' &&
          first.expression.callee.type === 'FunctionExpression'
        ) {
          return;
        }

        const names = new Set();
        for (const stmt of path.node.body) {
          collectBindingNames(stmt, names);
        }

        const exposeStmts = [];
        for (const name of names) {
          // window.X = X
          exposeStmts.push(
            t.expressionStatement(
              t.assignmentExpression(
                '=',
                t.memberExpression(t.identifier('window'), t.identifier(name)),
                t.identifier(name),
              ),
            ),
          );
        }

        const body = [...path.node.body, ...exposeStmts];
        const iife = t.expressionStatement(
          t.callExpression(
            t.functionExpression(null, [], t.blockStatement(body)),
            [],
          ),
        );
        path.node.body = [iife];
      },
    },
  },
});

function collectBindingNames(stmt, out) {
  switch (stmt.type) {
    case 'FunctionDeclaration':
    case 'ClassDeclaration':
      if (stmt.id && stmt.id.name) out.add(stmt.id.name);
      break;
    case 'VariableDeclaration':
      for (const decl of stmt.declarations) {
        // Skip destructuring patterns — they are imports
        // (e.g., `const { tFor } = window.cc`, `const { useState } = React`),
        // not exports. Auto-exposing would overwrite the real window globals
        // with locally-undefined values when the source object is missing.
        if (decl.id.type === 'Identifier') {
          out.add(decl.id.name);
        }
      }
      break;
    default:
      break;
  }
}

function addPatternNames(node, out) {
  if (!node) return;
  switch (node.type) {
    case 'Identifier':
      out.add(node.name);
      break;
    case 'ObjectPattern':
      for (const prop of node.properties) {
        if (prop.type === 'ObjectProperty' || prop.type === 'Property') {
          addPatternNames(prop.value, out);
        } else if (prop.type === 'RestElement') {
          addPatternNames(prop.argument, out);
        }
      }
      break;
    case 'ArrayPattern':
      for (const el of node.elements) {
        addPatternNames(el, out);
      }
      break;
    case 'AssignmentPattern':
      addPatternNames(node.left, out);
      break;
    case 'RestElement':
      addPatternNames(node.argument, out);
      break;
    default:
      break;
  }
}

module.exports = {
  presets: [['@babel/preset-react', { runtime: 'classic' }]],
  plugins: [wrapInIife],
  sourceMaps: 'inline',
};
