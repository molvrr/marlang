import { For } from 'solid-js';
import type { BuiltinReference } from '@marlang/browser';

interface ReferencePanelProps {
  reference: BuiltinReference;
}

export default function ReferencePanel(props: ReferencePanelProps) {
  return (
    <div class="reference-panel">
      <section class="reference-section">
        <h3>Built-in Functions</h3>
        <For each={props.reference.builtins}>
          {(category) => (
            <div class="reference-category">
              <h4>{category.name}</h4>
              <div class="reference-items">
                <For each={category.items}>
                  {(item) => (
                    <div class="reference-item">
                      <code class="ref-name">{item.name}</code>
                      <code class="ref-type">{item.type}</code>
                      {item.description && (
                        <span class="ref-desc">{item.description}</span>
                      )}
                    </div>
                  )}
                </For>
              </div>
            </div>
          )}
        </For>
      </section>

      <section class="reference-section">
        <h3>Data Types</h3>
        <For each={props.reference.dataTypes}>
          {(dataType) => (
            <div class="reference-category">
              <h4>
                {dataType.name}
                {dataType.typeParams.length > 0 && (
                  <span class="type-params"> {dataType.typeParams.join(' ')}</span>
                )}
              </h4>
              <div class="reference-items">
                <For each={dataType.constructors}>
                  {(ctor) => (
                    <div class="reference-item">
                      <code class="ref-name">{ctor.name}</code>
                      <code class="ref-type">{ctor.type}</code>
                      {ctor.description && (
                        <span class="ref-desc">{ctor.description}</span>
                      )}
                    </div>
                  )}
                </For>
              </div>
            </div>
          )}
        </For>
      </section>

      <section class="reference-section">
        <h3>Type Classes</h3>
        <For each={props.reference.typeClasses}>
          {(tc) => (
            <div class="reference-category">
              <h4>
                {tc.name} {tc.typeParam}
                {tc.superclasses.length > 0 && (
                  <span class="superclasses"> (extends {tc.superclasses.join(', ')})</span>
                )}
              </h4>
              <div class="reference-items">
                <For each={tc.methods}>
                  {(method) => (
                    <div class="reference-item">
                      <code class="ref-name">{method.name}</code>
                      <code class="ref-type">{method.type}</code>
                    </div>
                  )}
                </For>
              </div>
            </div>
          )}
        </For>
      </section>

      <section class="reference-section">
        <h3>Effect Syntax</h3>
        <div class="reference-items">
          <For each={props.reference.effectSyntax}>
            {(item) => (
              <div class="reference-item effect-item">
                <code class="ref-name">{item.name}</code>
                <code class="ref-type">{item.syntax}</code>
                <span class="ref-desc">{item.description}</span>
                {item.example && (
                  <pre class="ref-example"><code>{item.example}</code></pre>
                )}
              </div>
            )}
          </For>
        </div>
      </section>
    </div>
  );
}
