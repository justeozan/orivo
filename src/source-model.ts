import type {
  ConnectedSource,
  GameSource,
  SourceAccountStatus,
  SourceConnectStyle,
  SourceSyncResult,
} from "./contracts";
import type { IconName } from "./icons";

/**
 * The presentation half of the connected-store connectors. The backend owns
 * which stores exist and whether each is signed in; this owns how they read.
 *
 * The order is the order the Settings list and the Sources menu use, so the
 * two never disagree about where a store sits.
 */
export interface ConnectedSourceDescriptor {
  provider: ConnectedSource;
  label: string;
  icon: IconName;
}

export const CONNECTED_SOURCES: readonly ConnectedSourceDescriptor[] = [
  { provider: "epic", label: "Epic Games", icon: "epic" },
  { provider: "gog", label: "GOG", icon: "gog" },
  { provider: "ubisoft", label: "Ubisoft Connect", icon: "ubisoft" },
  { provider: "xbox", label: "Xbox", icon: "xbox" },
  { provider: "microsoft-store", label: "Microsoft Store", icon: "microsoft" },
  { provider: "instant-gaming", label: "Instant Gaming", icon: "instant-gaming" },
] as const;

const PROVIDERS = CONNECTED_SOURCES.map((source) => source.provider);
const CONNECT_STYLES: readonly SourceConnectStyle[] = ["token", "session"];

export function isConnectedSource(value: string): value is ConnectedSource {
  return (PROVIDERS as string[]).includes(value);
}

export function connectedSourceDescriptor(
  provider: ConnectedSource,
): ConnectedSourceDescriptor {
  return (
    CONNECTED_SOURCES.find((source) => source.provider === provider) ??
    CONNECTED_SOURCES[0]
  );
}

/**
 * The badge one library game wears. Every source the catalog can return has an
 * answer here, so a newly connected store never falls back to "Local" and
 * quietly misattributes where a game came from.
 */
export function sourceBadge(
  source: GameSource | undefined,
): { label: string; icon: IconName } | null {
  switch (source) {
    case "steam":
      return { label: "Steam", icon: "steam" };
    case "wine":
      return { label: "Windows", icon: "windows" };
    case "local":
      return { label: "Local", icon: "folder" };
    case "epic":
      return { label: "Epic Games", icon: "epic" };
    case "gog":
      return { label: "GOG", icon: "gog" };
    case "ubisoft":
      return { label: "Ubisoft", icon: "ubisoft" };
    case "xbox":
      return { label: "Xbox", icon: "xbox" };
    case "microsoft-store":
      return { label: "Microsoft Store", icon: "microsoft" };
    case "instant-gaming":
      return { label: "Instant Gaming", icon: "instant-gaming" };
    default:
      return null;
  }
}

/** Every source status, in registry order, with a disconnected default. */
export function defaultSourceAccounts(): SourceAccountStatus[] {
  return CONNECTED_SOURCES.map((source) => ({
    provider: source.provider,
    label: source.label,
    description: "",
    connected: false,
    accountLabel: "",
    style: source.provider === "ubisoft" || source.provider === "instant-gaming"
      ? "session"
      : "token",
    sharesSignInWith: [],
    launchable: source.provider !== "xbox" && source.provider !== "instant-gaming",
  }));
}

/**
 * Read the backend's account list. A source the backend did not mention keeps
 * its disconnected default rather than disappearing from Settings, so a partial
 * answer never makes a store look unavailable.
 */
export function normaliseSourceAccounts(value: unknown): SourceAccountStatus[] {
  const byProvider = new Map(
    defaultSourceAccounts().map((status) => [status.provider, status]),
  );
  for (const candidate of Array.isArray(value) ? value : []) {
    const record = asRecord(candidate);
    const provider = readString(record.provider);
    if (!isConnectedSource(provider)) continue;
    const style = readString(record.style);
    const fallback = byProvider.get(provider)!;
    byProvider.set(provider, {
      provider,
      label: readString(record.label) || connectedSourceDescriptor(provider).label,
      description: readString(record.description),
      connected: record.connected === true,
      accountLabel: readString(record.accountLabel ?? record.account_label),
      style: CONNECT_STYLES.includes(style as SourceConnectStyle)
        ? (style as SourceConnectStyle)
        : fallback.style,
      sharesSignInWith: readStringArray(
        record.sharesSignInWith ?? record.shares_sign_in_with,
      ).filter(isConnectedSource),
      launchable: record.launchable === true,
    });
  }
  return CONNECTED_SOURCES.map((source) => byProvider.get(source.provider)!);
}

export function normaliseSourceSyncResult(value: unknown): SourceSyncResult | null {
  const record = asRecord(value);
  const provider = readString(record.provider);
  if (!isConnectedSource(provider)) return null;
  return {
    provider,
    label: readString(record.label) || connectedSourceDescriptor(provider).label,
    totalGames: nonNegativeNumber(record.totalGames ?? record.total_games),
    importedGames: nonNegativeNumber(record.importedGames ?? record.imported_games),
    updatedGames: nonNegativeNumber(record.updatedGames ?? record.updated_games),
    skippedGames: nonNegativeNumber(record.skippedGames ?? record.skipped_games),
  };
}

/**
 * One sentence describing what a sync actually did. It never rounds a partial
 * import up to a clean one: entries the store returned but Orivo could not read
 * are always named.
 */
export function sourceSyncSummary(result: SourceSyncResult): string {
  const games = (count: number): string => (count === 1 ? "1 game" : `${count} games`);
  const parts: string[] = [];
  if (result.importedGames > 0) parts.push(`${games(result.importedGames)} added`);
  if (result.updatedGames > 0) parts.push(`${games(result.updatedGames)} refreshed`);
  if (parts.length === 0) {
    parts.push(result.totalGames === 0 ? "no games found" : "nothing new");
  }
  if (result.skippedGames > 0) {
    parts.push(`${games(result.skippedGames)} could not be read`);
  }
  return `${result.label}: ${parts.join(" · ")}`;
}

/** How a connected source reads in the Sources menu and in Settings. */
export function sourceStatusLine(status: SourceAccountStatus): string {
  if (!status.connected) {
    return status.style === "session" ? "Signs in through its own window" : "Not connected";
  }
  const account = status.accountLabel.trim();
  return account ? `Connected as ${account}` : "Connected";
}

function asRecord(value: unknown): Record<string, unknown> {
  return typeof value === "object" && value !== null ? (value as Record<string, unknown>) : {};
}

function readString(value: unknown): string {
  return typeof value === "string" ? value : "";
}

function readStringArray(value: unknown): string[] {
  return Array.isArray(value) ? value.filter((entry): entry is string => typeof entry === "string") : [];
}

function nonNegativeNumber(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) ? Math.max(0, Math.floor(value)) : 0;
}
