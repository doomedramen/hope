import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, createFileRoute } from "@tanstack/react-router";
import { PlusIcon, ServerIcon } from "lucide-react";
import { useState } from "react";
import { DiscoveryScopeSetup } from "@/components/DiscoveryScopeSetup";
import { AddNetworkDialog } from "./infrastructure";
import {
  createNetwork,
  deleteNetwork,
  fetchNetworks,
  patchNetwork,
  type Network,
} from "@/lib/api";
import { Button } from "@/components/ui/button";

export const Route = createFileRoute("/networks")({
  component: NetworksPage,
});

export function NetworksPage() {
  const queryClient = useQueryClient();
  const [dialog, setDialog] = useState<"create" | "edit" | null>(null);
  const [networkTarget, setNetworkTarget] = useState<Network | null>(null);
  const networksQuery = useQuery({
    queryKey: ["networks"],
    queryFn: fetchNetworks,
  });
  const networkCreateMutation = useMutation({
    mutationFn: createNetwork,
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ["networks"] });
      setNetworkTarget(null);
      setDialog(null);
    },
  });
  const networkPatchMutation = useMutation({
    mutationFn: ({
      id,
      input,
    }: {
      id: string;
      input: Parameters<typeof patchNetwork>[1];
    }) => patchNetwork(id, input),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ["networks"] });
      setNetworkTarget(null);
      setDialog(null);
    },
  });
  const networkDeleteMutation = useMutation({
    mutationFn: deleteNetwork,
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ["networks"] });
      setNetworkTarget(null);
    },
  });
  const networks = networksQuery.data?.items ?? [];

  return (
    <div className="mx-auto flex max-w-7xl flex-col gap-6">
      <div className="flex flex-col justify-between gap-4 md:flex-row md:items-end">
        <div>
          <h1 className="text-3xl font-semibold tracking-tight">Networks</h1>
          <p className="mt-2 max-w-2xl text-sm text-muted-foreground">
            Define scan boundaries, review approved targets, and monitor every
            discovery job for each network.
          </p>
        </div>
        <div className="flex flex-wrap gap-2">
          <Link
            className="inline-flex min-h-9 items-center justify-center rounded-md border px-3 text-sm font-medium hover:bg-muted"
            to="/devices"
          >
            <ServerIcon className="mr-2 size-4" />
            View devices
          </Link>
          <Button
            onClick={() => {
              networkCreateMutation.reset();
              setNetworkTarget(null);
              setDialog("create");
            }}
          >
            <PlusIcon data-icon="inline-start" />
            Add network
          </Button>
        </div>
      </div>

      <DiscoveryScopeSetup
        actionError={networkDeleteMutation.error}
        error={networksQuery.error}
        loading={networksQuery.isLoading}
        networks={networks}
        onAddNetwork={() => {
          networkCreateMutation.reset();
          setNetworkTarget(null);
          setDialog("create");
        }}
        onDeleteNetwork={(network) => {
          if (
            window.confirm(
              `Delete ${network.name || network.cidr}? Dependent discovery data must be removed first.`,
            )
          ) {
            networkDeleteMutation.mutate(network.id);
          }
        }}
        onEditNetwork={(network) => {
          networkPatchMutation.reset();
          setNetworkTarget(network);
          setDialog("edit");
        }}
      />

      <AddNetworkDialog
        error={
          dialog === "edit"
            ? networkPatchMutation.error
            : networkCreateMutation.error
        }
        network={dialog === "edit" ? networkTarget : null}
        onOpenChange={(open) => {
          if (!open) {
            setNetworkTarget(null);
            setDialog(null);
          }
        }}
        open={dialog !== null}
        pending={
          networkCreateMutation.isPending || networkPatchMutation.isPending
        }
        submit={(input) => {
          if (dialog === "edit" && networkTarget) {
            networkPatchMutation.mutate({
              id: networkTarget.id,
              input: {
                version: networkTarget.version,
                cidr: input.cidr,
                gateway: input.gateway ?? null,
                name: input.name ?? null,
                vlan: input.vlan ?? null,
              },
            });
          } else {
            networkCreateMutation.mutate(input);
          }
        }}
      />
    </div>
  );
}
