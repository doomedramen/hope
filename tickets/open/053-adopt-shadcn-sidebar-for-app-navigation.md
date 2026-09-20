---
id: "053"
title: "Adopt the shadcn sidebar for responsive app navigation"
status: open
priority: medium
created: "2026-09-20T22:57:48Z"
updated: "2026-09-20T22:57:48Z"
tags: ["navigation", "sidebar", "mobile", "design-system", "ux"]
---

## Problem

The application shell implements navigation twice: a hand-built desktop `aside` and a separate two-column mobile grid. The duplicated markup has different spacing, states, and responsive behavior, and the phone version consumes a large portion of the initial viewport. It also leaves no compact way to reach navigation while keeping the current page as the primary surface.

## Context

The project already includes shadcn/ui's `sidebar` component. Replace the custom shell with one `SidebarProvider` and an `AppSidebar`: use `SidebarHeader` for Hope branding, `SidebarContent` plus `SidebarMenu` for the route links, and `SidebarFooter` for health or account controls. Pair it with `SidebarInset` and a visible `SidebarTrigger` in the compact header. On desktop, support an intentional expanded or icon-collapsed state; on phones, use the component's off-canvas mobile behavior and close it after route selection. Preserve the skip link, current-route indication, keyboard focus treatment, and the existing device search entry point.
