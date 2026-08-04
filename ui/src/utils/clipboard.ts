/**
 * Clipboard write with a fallback for non-secure contexts.
 *
 * `navigator.clipboard` is undefined on plain-HTTP origins, which is exactly
 * how the browser UI is served when the engine runs on a LAN address rather
 * than localhost. Without the fallback the copy buttons silently fail in the
 * one setup where a user is most likely to be sharing findings.
 *
 * Extracted from the near-identical block in `QualityReport.svelte`; the
 * hidden-textarea path is the legacy `execCommand('copy')` dance, which stays
 * the only option there.
 */
export async function copyToClipboard(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    // Permission denied or insecure context — fall through to the textarea.
  }

  const ta = document.createElement('textarea');
  ta.value = text;
  // Keep it off-screen but focusable: `display: none` can't be selected.
  ta.style.position = 'fixed';
  ta.style.opacity = '0';
  ta.style.pointerEvents = 'none';
  document.body.appendChild(ta);
  ta.select();
  let ok = false;
  try {
    ok = document.execCommand('copy');
  } catch {
    ok = false;
  }
  document.body.removeChild(ta);
  return ok;
}
