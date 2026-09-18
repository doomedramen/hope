import { createFileRoute } from "@tanstack/react-router";

export const Route = createFileRoute("/settings")({
  component: SettingsPage,
});

function SettingsPage() {
  return (
    <div>
      <h1 className="text-xl font-semibold">Settings</h1>
      <p className="mt-2 text-sm text-slate-500">Coming in a later milestone.</p>
    </div>
  );
}
