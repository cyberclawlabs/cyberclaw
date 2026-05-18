// useModalA11y — focus trap + Escape-to-close + restore-on-close for any
// dialog-shaped component. Extracted from Modal.tsx so ad-hoc inline
// modals (CronPage NewJobDialog / AgentsPage HandoffDrawer detail /
// ProfilesPage editor / OnboardingBanner) can opt into the same a11y
// guarantees without a full structural refactor to <Modal>.
//
// Usage:
//   const ref = useRef<HTMLDivElement>(null);
//   useModalA11y(ref, open, onClose);
//   return open ? (
//     <div className="fixed inset-0 …" onClick={…}>
//       <div ref={ref} role="dialog" aria-modal="true" aria-label={title}
//            className="…" tabIndex={-1}>
//         …
//       </div>
//     </div>
//   ) : null;
//
// The hook is a noop when `open` is false.

import { useEffect, type RefObject } from "react";

const FOCUSABLE = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled])",
  "textarea:not([disabled])",
  "select:not([disabled])",
  '[tabindex]:not([tabindex="-1"])',
].join(",");

export function useModalA11y(
  ref: RefObject<HTMLElement | null>,
  open: boolean,
  onClose: () => void,
): void {
  // Escape to close (window-level so it works whether or not the
  // dialog has focus).
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  // Focus trap + initial focus + restore previously-focused element on close.
  useEffect(() => {
    if (!open || !ref.current) return;
    const container = ref.current;
    const previouslyFocused = document.activeElement as HTMLElement | null;
    const focusables = () =>
      Array.from(container.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
        (el) => !el.hasAttribute("data-modal-skip-focus") && el.offsetParent !== null,
      );
    const list = focusables();
    list[0]?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Tab") return;
      const items = focusables();
      if (items.length === 0) return;
      const first = items[0]!;
      const last = items[items.length - 1]!;
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    };
    container.addEventListener("keydown", onKey);
    return () => {
      container.removeEventListener("keydown", onKey);
      previouslyFocused?.focus?.();
    };
    // Include onClose for symmetry with the Escape effect above —
    // future hook expansions that call onClose from inside the trap
    // won't capture a stale reference.
  }, [open, ref, onClose]);
}
