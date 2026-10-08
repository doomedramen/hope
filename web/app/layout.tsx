import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = { title: "H.O.P.E — Your network, understood", description: "Local network discovery and host monitoring" };
export default function RootLayout({ children }: Readonly<{ children: React.ReactNode }>) {
  return <html lang="en"><body>{children}</body></html>;
}
