import { createContext, useCallback, useContext, useState } from "react";
import type { ReactNode } from "react";

export type ToastTone = "success" | "error" | "info";
type Toast = { id: number; tone: ToastTone; msg: string };
type ToastApi = (input: { tone?: ToastTone; msg: string }) => void;

const Ctx = createContext<ToastApi>(() => {});

const TONE_CLS: Record<ToastTone, string> = {
  success: "bg-emerald-500/15 border-emerald-500/30 text-emerald-200",
  error: "bg-rose-500/15 border-rose-500/30 text-rose-200",
  info: "bg-bg-2 border-border text-fg",
};

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);

  const toast = useCallback<ToastApi>((input) => {
    const id = Date.now() + Math.random();
    setToasts((prev) => [...prev, { id, tone: input.tone ?? "info", msg: input.msg }]);
    setTimeout(() => setToasts((prev) => prev.filter((t) => t.id !== id)), 4500);
  }, []);

  return (
    <Ctx.Provider value={toast}>
      {children}
      <div className="fixed bottom-4 right-4 z-[70] flex flex-col gap-2 pointer-events-none">
        {toasts.map((t) => (
          <div
            key={t.id}
            className={`px-3 py-2 rounded-md text-[13px] border pointer-events-auto max-w-md anim-slide-up ${TONE_CLS[t.tone]}`}
          >
            {t.msg}
          </div>
        ))}
      </div>
    </Ctx.Provider>
  );
}

export function useToast(): ToastApi {
  return useContext(Ctx);
}
