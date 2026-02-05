import { For, Show, createSignal } from 'solid-js';
import type { BuiltinReference } from '@marlang/browser';

type RefTab = 'core' | 'effects' | 'concurrency' | 'stdlib';

interface ReferencePanelProps {
  reference: BuiltinReference;
}

export default function ReferencePanel(props: ReferencePanelProps) {
  const [tab, setTab] = createSignal<RefTab>('core');

  const coreBuiltins = () =>
    props.reference.builtins.filter((c) => c.name !== 'Concurrency');

  const concurrencyBuiltins = () =>
    props.reference.builtins
      .filter((c) => c.name === 'Concurrency')
      .flatMap((c) => c.items);

  const tabs: { id: RefTab; label: string }[] = [
    { id: 'core', label: 'Core' },
    { id: 'effects', label: 'Effects' },
    { id: 'concurrency', label: 'Concurrency' },
    { id: 'stdlib', label: 'Stdlib' },
  ];

  return (
    <div class="reference-panel">
      <div class="ref-tabs">
        <For each={tabs}>
          {(t) => (
            <button
              class={tab() === t.id ? 'active' : ''}
              onClick={() => setTab(t.id)}
            >
              {t.label}
            </button>
          )}
        </For>
      </div>

      <div class="ref-content">
        <Show when={tab() === 'core'}>
          <section class="reference-section">
            <h3>Built-in Functions</h3>
            <For each={coreBuiltins()}>
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
        </Show>

        <Show when={tab() === 'effects'}>
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
        </Show>

        <Show when={tab() === 'concurrency'}>
          <Show when={concurrencyBuiltins().length > 0}>
            <section class="reference-section">
              <h3>Built-ins</h3>
              <div class="reference-items">
                <For each={concurrencyBuiltins()}>
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
            </section>
          </Show>
          <section class="reference-section">
            <h3>Concurrency</h3>
            <div class="reference-items">
              <For each={props.reference.concurrency}>
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
        </Show>

        <Show when={tab() === 'stdlib'}>
          <section class="reference-section">
            <h3>Standard Library</h3>
            <For each={props.reference.stdlib}>
              {(mod) => (
                <div class="reference-category">
                  <h4>
                    {mod.name}
                    <code class="ref-import">(import "{mod.importPath}" as ...)</code>
                  </h4>
                  <span class="ref-desc">{mod.description}</span>
                  <div class="reference-items">
                    <For each={mod.items}>
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
        </Show>
      </div>
    </div>
  );
}
