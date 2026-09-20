import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import { CircleAlertIcon, PlusIcon } from "lucide-react";
import { useState, type FormEvent } from "react";
import {
  createNotificationChannel,
  createNotificationRoute,
  fetchNotificationChannels,
  fetchNotificationRoutes,
  type NotificationProvider,
} from "@/lib/api";
import { formatDate, formatRelative } from "@/lib/format";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardAction,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyTitle,
} from "@/components/ui/empty";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { Spinner } from "@/components/ui/spinner";

export const Route = createFileRoute("/settings")({
  component: SettingsPage,
});

function SettingsPage() {
  const queryClient = useQueryClient();
  const channelsQuery = useQuery({
    queryKey: ["notification-channels"],
    queryFn: fetchNotificationChannels,
  });
  const routesQuery = useQuery({
    queryKey: ["notification-routes"],
    queryFn: fetchNotificationRoutes,
  });
  const channels = channelsQuery.data?.items ?? [];
  const routes = routesQuery.data?.items ?? [];
  const [name, setName] = useState("");
  const [provider, setProvider] = useState<NotificationProvider>("webhook");
  const [url, setUrl] = useState("");
  const [topic, setTopic] = useState("");
  const [token, setToken] = useState("");
  const [minSeverity, setMinSeverity] = useState("warning");
  const [delaySeconds, setDelaySeconds] = useState("0");
  const createMutation = useMutation({
    mutationFn: async () => {
      const channel = await createNotificationChannel({
        name,
        provider,
        config: {
          url,
          ...(provider === "ntfy" ? { topic } : {}),
          ...(token ? { token } : {}),
        },
      });
      await createNotificationRoute({
        channel_id: channel.id,
        min_severity: minSeverity,
        event_types: ["incident.opened", "incident.recovered"],
        delay_seconds: Number(delaySeconds) || 0,
      });
      return channel;
    },
    onSuccess: async () => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["notification-channels"] }),
        queryClient.invalidateQueries({ queryKey: ["notification-routes"] }),
      ]);
      setName("");
      setUrl("");
      setTopic("");
      setToken("");
      setDelaySeconds("0");
    },
  });

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    createMutation.mutate();
  }

  return (
    <div className="mx-auto flex max-w-5xl flex-col gap-6">
      <h1 className="text-3xl font-semibold tracking-tight">Settings</h1>
      {channelsQuery.isError || routesQuery.isError ? (
        <Alert aria-live="assertive" variant="destructive">
          <CircleAlertIcon />
          <AlertTitle>Notification settings unavailable</AlertTitle>
          <AlertDescription>
            {channelsQuery.error instanceof Error
              ? channelsQuery.error.message
              : routesQuery.error instanceof Error
                ? routesQuery.error.message
                : "Notification settings could not be loaded."}
          </AlertDescription>
          <Button
            onClick={() => {
              void channelsQuery.refetch();
              void routesQuery.refetch();
            }}
            size="sm"
            variant="outline"
          >
            Retry
          </Button>
        </Alert>
      ) : null}
      <Card>
        <CardHeader className="border-b max-sm:grid-cols-1">
          <CardTitle>Notification channels</CardTitle>
          <CardAction className="max-sm:col-start-1 max-sm:row-start-2 max-sm:justify-self-start">
            <Badge variant={channels.length ? "outline" : "secondary"}>
              {channels.length}
            </Badge>
          </CardAction>
        </CardHeader>
        <CardContent className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_minmax(18rem,0.8fr)]">
          <div aria-busy={channelsQuery.isLoading} className="min-w-0 divide-y">
            {channelsQuery.isLoading ? (
              renderSettingsListLoading()
            ) : channels.length === 0 ? (
              <Empty className="min-h-32 border-0 p-4">
                <EmptyHeader>
                  <EmptyTitle>No notification channels</EmptyTitle>
                  <EmptyDescription>
                    Add a channel to route incident notifications.
                  </EmptyDescription>
                </EmptyHeader>
              </Empty>
            ) : (
              channels.map((channel) => (
                <div
                  className="flex items-start justify-between gap-4 py-4"
                  key={channel.id}
                >
                  <div className="min-w-0">
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="font-medium">{channel.name}</span>
                      <Badge variant="secondary">{channel.provider}</Badge>
                      {channel.enabled ? null : (
                        <Badge variant="outline">Disabled</Badge>
                      )}
                    </div>
                    <p className="mt-1 truncate text-xs text-muted-foreground">
                      {typeof channel.config.url === "string"
                        ? channel.config.url
                        : "URL hidden"}
                    </p>
                  </div>
                  <span className="shrink-0 text-xs text-muted-foreground">
                    {formatRelative(channel.created_at)}
                  </span>
                </div>
              ))
            )}
          </div>
          <form
            className="flex flex-col gap-4 rounded-lg border p-4"
            onSubmit={submit}
          >
            <div className="flex items-center gap-2 font-medium">
              <PlusIcon className="size-4" />
              Add channel
            </div>
            <div className="grid gap-2">
              <Label htmlFor="notification-name">Name</Label>
              <Input
                id="notification-name"
                onChange={(event) => setName(event.target.value)}
                required
                value={name}
              />
            </div>
            <div className="grid gap-2">
              <Label htmlFor="notification-provider">Provider</Label>
              <Select
                onValueChange={(value) => {
                  if (value) setProvider(value as NotificationProvider);
                }}
                value={provider}
              >
                <SelectTrigger id="notification-provider">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="webhook">Webhook</SelectItem>
                  <SelectItem value="ntfy">ntfy</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <div className="grid gap-2">
              <Label htmlFor="notification-url">URL</Label>
              <Input
                id="notification-url"
                onChange={(event) => setUrl(event.target.value)}
                placeholder="https://..."
                required
                type="url"
                value={url}
              />
            </div>
            {provider === "ntfy" ? (
              <div className="grid gap-2">
                <Label htmlFor="notification-topic">Topic</Label>
                <Input
                  id="notification-topic"
                  onChange={(event) => setTopic(event.target.value)}
                  required
                  value={topic}
                />
              </div>
            ) : null}
            <div className="grid gap-2">
              <Label htmlFor="notification-token">Token</Label>
              <Input
                id="notification-token"
                onChange={(event) => setToken(event.target.value)}
                type="password"
                value={token}
              />
            </div>
            <div className="grid gap-2 sm:grid-cols-2">
              <div className="grid gap-2">
                <Label htmlFor="notification-severity">Minimum severity</Label>
                <Select
                  onValueChange={(value) => {
                    if (value) setMinSeverity(value);
                  }}
                  value={minSeverity}
                >
                  <SelectTrigger id="notification-severity">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="info">Info</SelectItem>
                    <SelectItem value="notice">Notice</SelectItem>
                    <SelectItem value="warning">Warning</SelectItem>
                    <SelectItem value="critical">Critical</SelectItem>
                  </SelectContent>
                </Select>
              </div>
              <div className="grid gap-2">
                <Label htmlFor="notification-delay">Delay seconds</Label>
                <Input
                  id="notification-delay"
                  min="0"
                  max="86400"
                  onChange={(event) => setDelaySeconds(event.target.value)}
                  type="number"
                  value={delaySeconds}
                />
              </div>
            </div>
            {createMutation.isError ? (
              <Alert aria-live="assertive" variant="destructive">
                <CircleAlertIcon />
                <AlertTitle>Channel was not created</AlertTitle>
                <AlertDescription>
                  {createMutation.error instanceof Error
                    ? createMutation.error.message
                    : "Try again."}
                </AlertDescription>
              </Alert>
            ) : null}
            <Button disabled={createMutation.isPending} type="submit">
              {createMutation.isPending ? (
                <Spinner data-icon="inline-start" />
              ) : null}
              {createMutation.isPending ? "Saving..." : "Save channel"}
            </Button>
          </form>
        </CardContent>
      </Card>
      <Card>
        <CardHeader className="border-b max-sm:grid-cols-1">
          <CardTitle>Notification routes</CardTitle>
          <CardAction className="max-sm:col-start-1 max-sm:row-start-2 max-sm:justify-self-start">
            <Badge variant={routes.length ? "outline" : "secondary"}>
              {routes.length}
            </Badge>
          </CardAction>
        </CardHeader>
        <CardContent>
          {routesQuery.isLoading ? (
            renderSettingsListLoading()
          ) : routes.length === 0 ? (
            <Empty className="min-h-32 border-0 p-4">
              <EmptyHeader>
                <EmptyTitle>No notification routes</EmptyTitle>
                <EmptyDescription>
                  Routes appear after you add a notification channel.
                </EmptyDescription>
              </EmptyHeader>
            </Empty>
          ) : (
            <div className="divide-y">
              {routes.map((route) => (
                <div
                  className="flex flex-wrap items-center gap-2 py-3"
                  key={route.id}
                >
                  <span className="font-medium">{route.channel_name}</span>
                  <Badge variant="outline">{route.min_severity}+</Badge>
                  {route.event_types.map((eventType) => (
                    <Badge key={eventType} variant="secondary">
                      {eventType}
                    </Badge>
                  ))}
                  {route.delay_seconds ? (
                    <span className="text-xs text-muted-foreground">
                      {route.delay_seconds}s delay
                    </span>
                  ) : null}
                  <span
                    className="ml-auto text-xs text-muted-foreground"
                    title={formatDate(route.created_at)}
                  >
                    {formatRelative(route.created_at)}
                  </span>
                </div>
              ))}
            </div>
          )}
        </CardContent>
      </Card>
    </div>
  );
}

function renderSettingsListLoading() {
  return (
    <div
      aria-label="Loading notification settings"
      className="flex flex-col gap-3 py-4"
      role="status"
    >
      {Array.from({ length: 3 }, (_, index) => (
        <Skeleton className="h-12 w-full" key={index} />
      ))}
    </div>
  );
}
