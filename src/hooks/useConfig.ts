import { useAppContext } from "../context/AppContext";
import type { AppConfig } from "../types/config";

export function useConfig(): AppConfig {
  const { config } = useAppContext();
  return config;
}
