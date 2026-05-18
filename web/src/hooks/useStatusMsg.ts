import { useEffect, useRef, useState } from "react";

export function useStatusMsg() {
  const [msg, setMsg] = useState<{ text: string; ok: boolean } | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const show = (text: string, ok: boolean) => {
    if (timer.current) clearTimeout(timer.current);
    setMsg({ text, ok });
    timer.current = setTimeout(() => setMsg(null), 4500);
  };
  useEffect(() => () => { if (timer.current) clearTimeout(timer.current); }, []);
  return { msg, show };
}
