export type NetworkField = "cidr" | "gateway" | "vlan";

function validIpv4(value: string) {
  const parts = value.split(".");
  return (
    parts.length === 4 &&
    parts.every(
      (part) =>
        /^(0|[1-9]\d{0,2})$/.test(part) &&
        Number.parseInt(part, 10) >= 0 &&
        Number.parseInt(part, 10) <= 255,
    )
  );
}

function validIpv6(value: string) {
  if (value.includes("%")) return false;
  const halves = value.split("::");
  if (halves.length > 2) return false;
  const groups = halves.flatMap((half) => (half ? half.split(":") : []));
  if (groups.some((group) => !/^[0-9a-f]{1,4}$/i.test(group))) return false;
  return halves.length === 2 ? groups.length < 8 : groups.length === 8;
}

export function isValidIp(value: string) {
  return value.includes(":") ? validIpv6(value) : validIpv4(value);
}

export function isValidCidr(value: string) {
  const [address, prefix, ...rest] = value.trim().split("/");
  if (!address || !prefix || rest.length) return false;
  if (!isValidIp(address) || !/^\d+$/.test(prefix)) return false;
  const maxPrefix = address.includes(":") ? 128 : 32;
  const prefixLength = Number.parseInt(prefix, 10);
  return prefixLength >= 0 && prefixLength <= maxPrefix;
}

export function validateNetworkFields(values: {
  cidr: string;
  gateway: string;
  vlan: string;
}): Partial<Record<NetworkField, string>> {
  const errors: Partial<Record<NetworkField, string>> = {};
  if (!isValidCidr(values.cidr)) {
    errors.cidr = "CIDR must be a valid network range.";
  }
  if (values.gateway.trim() && !isValidIp(values.gateway.trim())) {
    errors.gateway = "Gateway must be a valid IP address.";
  }
  if (values.vlan.trim()) {
    const vlan = Number.parseInt(values.vlan, 10);
    if (!/^\d+$/.test(values.vlan.trim()) || vlan < 1 || vlan > 4094) {
      errors.vlan = "VLAN must be between 1 and 4094.";
    }
  }
  return errors;
}
