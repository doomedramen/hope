"use client";
import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useRef,
  useState,
} from "react";
import { CircleAlert, CircleCheck, Info, TriangleAlert, X } from "lucide-react";
import type { Tone } from "@/lib/fleet";

type Toast = { id: number; message: string; tone: Tone };
type Push = (message: string, tone?: Tone) => void;
const ToastContext = createContext<Push>(() => {});

export function useToast() {
  return useContext(ToastContext);
}

const ICONS = {
  good: CircleCheck,
  warning: TriangleAlert,
  critical: CircleAlert,
  neutral: Info,
};

export function ToastProvider({ children }: { children: React.ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const counter = useRef(0);
  const dismiss = useCallback(
    (id: number) => setToasts((all) => all.filter((t) => t.id !== id)),
    [],
  );
  const push = useCallback<Push>(
    (message, tone = "neutral") => {
      const id = ++counter.current;
      setToasts((all) => [...all.slice(-3), { id, message, tone }]);
      window.setTimeout(() => dismiss(id), tone === "critical" ? 8000 : 4500);
    },
    [dismiss],
  );
  const value = useMemo(() => push, [push]);
  return (
    <ToastContext.Provider value={value}>
      {children}
      <div className="toast-stack" aria-live="polite" aria-relevant="additions">
        {toasts.map((toast) => {
          const Icon = ICONS[toast.tone];
          return (
            <div key={toast.id} className="toast" data-tone={toast.tone}>
              <Icon size={16} />
              <span>{toast.message}</span>
              <button aria-label="Dismiss" onClick={() => dismiss(toast.id)}>
                <X size={14} />
              </button>
            </div>
          );
        })}
      </div>
    </ToastContext.Provider>
  );
}
