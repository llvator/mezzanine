<script lang="ts">
  /**
   * Connect screen (UI-034): choose, verify and remember an engine.
   *
   * Same shape as the serve-mode repo picker: decide what you are looking at
   * *before* the app tries to load anything. After UI-032 the app can talk to
   * any engine, but the only way to say which one was a query parameter —
   * undiscoverable, lost on the next visit, and silent about why nothing
   * loaded.
   *
   * The failure messages are the substance here. A refused origin is the one
   * a user cannot diagnose from the browser, and its remedy is a flag on a
   * command they have already run, so the screen prints that command ready to
   * copy rather than describing it.
   */
  import { connectTo, probing, type Connection } from '../stores/connection';
  import { parseEndpointInput } from '../endpoint';

  /** The verdict that put this screen up. */
  export let connection: Connection;
  /** The endpoint that produced it, `''` for same-origin. */
  export let attemptedBase: string;
  /** Called once an endpoint has answered; the parent boots the app. */
  export let onConnected: () => void;

  let input = attemptedBase || '';
  let token = '';
  let invalid = false;
  /** Cleared on edit so a stale verdict doesn't sit under a new value. */
  let result: Connection | null = connection;

  /** The origin this page is served from — what the engine has to allow. */
  const pageOrigin = window.location.origin;

  $: parsed = parseEndpointInput(input);
  $: showTokenField =
    result?.kind === 'token-required' || token.length > 0;

  async function connect() {
    const base = parseEndpointInput(input);
    if (!base) {
      invalid = true;
      return;
    }
    invalid = false;
    result = null;
    const verdict = await connectTo(base, token.trim() || null);
    result = verdict;
    if (verdict.kind === 'ok') onConnected();
  }

  function onInput() {
    invalid = false;
    result = null;
  }
</script>

<div class="connect" data-probe="connect-screen">
  <header>
    <h1>Mezzanine</h1>
    <p class="tagline">Point this page at a running engine.</p>
  </header>

  <form
    class="card"
    on:submit|preventDefault={connect}
    data-probe="connect-form"
  >
    <label for="endpoint">Engine address</label>
    <div class="row">
      <input
        id="endpoint"
        bind:value={input}
        on:input={onInput}
        placeholder="3200"
        autocomplete="off"
        spellcheck="false"
        data-probe="connect-input"
      />
      <button type="submit" disabled={$probing || !input.trim()} data-probe="connect-submit">
        {$probing ? 'Checking…' : 'Connect'}
      </button>
    </div>
    <p class="hint">
      A port (<code>3200</code>) or a full origin
      (<code>http://localhost:3200</code>). Start one with
      <code>mezz watch . --port 3200</code>.
      {#if parsed && parsed !== input.trim()}
        <br />Will connect to <code>{parsed}</code>.
      {/if}
    </p>

    {#if showTokenField}
      <label for="token">Pairing token</label>
      <input
        id="token"
        bind:value={token}
        on:input={onInput}
        placeholder="from the engine's startup banner"
        autocomplete="off"
        spellcheck="false"
        data-probe="connect-token"
      />
    {/if}

    {#if invalid}
      <p class="problem" data-probe="connect-error" data-connect-error="invalid">
        That isn't a port or an origin. Try <code>3200</code> or
        <code>http://localhost:3200</code>.
      </p>
    {:else if result && result.kind !== 'ok'}
      <div class="problem" data-probe="connect-error" data-connect-error={result.kind}>
        {#if result.kind === 'unreachable'}
          <strong>Nothing is listening there.</strong>
          <p>Check the port, and that the engine is still running:</p>
          <pre>mezz watch . --port {parsed ? new URL(parsed).port || '80' : '3200'}</pre>
        {:else if result.kind === 'not-mezz'}
          <strong>Something is listening, but it isn't mezz.</strong>
          <p>
            That port belongs to another server. Check which port the engine
            printed at startup — it is not always the one you last used.
          </p>
        {:else if result.kind === 'refused'}
          <strong>The engine is there, and it refused this page.</strong>
          <p>
            It only answers pages it served itself, or origins you name. Restart
            it with this origin allowed:
          </p>
          <pre>mezz watch . --allow-origin {pageOrigin}</pre>
          <p class="why">
            Loopback is not a boundary against a browser on the same machine,
            which is why the engine asks rather than assuming.
          </p>
        {:else if result.kind === 'token-required'}
          <strong>The engine wants its pairing token.</strong>
          <p>
            This origin isn't loopback, so being allowed isn't enough on its
            own. Copy the token from the engine's startup banner into the field
            above. It is new on every run.
          </p>
        {/if}
      </div>
    {/if}
  </form>
</div>

<style>
  .connect {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 24px;
    height: 100vh;
    width: 100vw;
    padding: 32px;
    overflow-y: auto;
    background: var(--bg-deep);
    color: var(--text);
  }

  header { text-align: center; }
  header h1 { margin: 0; color: var(--accent); }
  .tagline { margin: 6px 0 0; color: var(--text-muted); font-size: 0.9rem; }

  .card {
    width: 100%;
    max-width: 520px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 20px;
  }

  label {
    font-size: 0.75rem;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-muted);
  }

  .row { display: flex; gap: 8px; }

  input {
    flex: 1;
    background: var(--bg-deep);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 8px 10px;
    color: var(--text);
    font: inherit;
    font-size: 0.9rem;
  }
  input:focus { outline: none; border-color: var(--accent); }

  button {
    background: var(--accent);
    border: none;
    border-radius: 4px;
    padding: 8px 18px;
    color: var(--bg-deep);
    font: inherit;
    font-size: 0.85rem;
    font-weight: 600;
    cursor: pointer;
  }
  button:disabled { opacity: 0.5; cursor: default; }

  .hint { margin: 0; color: var(--text-muted); font-size: 0.78rem; line-height: 1.5; }

  .problem {
    margin: 4px 0 0;
    padding: 12px 14px;
    border-radius: 4px;
    border: 1px solid var(--border);
    background: var(--bg-deep);
    color: var(--text-secondary);
    font-size: 0.8rem;
    line-height: 1.55;
  }
  .problem strong { display: block; color: var(--text); margin-bottom: 4px; }
  .problem p { margin: 6px 0 0; }
  .problem .why { color: var(--text-dim, #666); font-size: 0.75rem; }

  pre {
    margin: 8px 0 0;
    padding: 8px 10px;
    border-radius: 4px;
    background: var(--bg-surface);
    border: 1px solid var(--border);
    overflow-x: auto;
    font-family: 'Monaco', 'Menlo', monospace;
    font-size: 0.75rem;
    color: var(--text);
    user-select: all;
  }

  code {
    background: var(--bg-deep);
    padding: 1px 5px;
    border-radius: 3px;
    font-family: 'Monaco', 'Menlo', monospace;
    font-size: 0.75rem;
  }
</style>
