"use client";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from "@/components/ui/dialog";
import { ChangePassword } from "@/components/change-password";

export function AccountDialog({
  open,
  onOpenChange,
  onPasswordChanged,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onPasswordChanged: () => void;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Account</DialogTitle>
          <DialogDescription>
            The administrator account for this H.O.P.E server.
          </DialogDescription>
        </DialogHeader>
        <section className="flex flex-col gap-3">
          <h3 className="text-sm font-semibold">Password</h3>
          {/* Remounted on every open so a half-typed password never lingers. */}
          {open && (
            <ChangePassword
              onChanged={() => {
                onOpenChange(false);
                onPasswordChanged();
              }}
            />
          )}
        </section>
      </DialogContent>
    </Dialog>
  );
}
