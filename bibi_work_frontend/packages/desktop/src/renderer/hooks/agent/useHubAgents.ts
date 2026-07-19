import { useState, useEffect, useCallback } from 'react';
import type { HubStateChange, IHubAgentItem } from '@/common/types/agent/hub';
import { ipcBridge } from '@/common';
import { refreshManagedAgentCatalogAndAssistants } from './useManagedAgents';

export function useHubAgents() {
  const [agents, setAgents] = useState<IHubAgentItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string>();

  const applyStateChange = useCallback((payload: HubStateChange) => {
    setAgents((prev) =>
      prev.map((agent) => {
        if (agent.name === payload.name) {
          return {
            ...agent,
            status: payload.status,
            installError: payload.error,
          };
        }
        return agent;
      })
    );

    // Hub installs can change both the management diagnostics view and
    // the generated assistant catalog, so refresh both through the
    // shared helper used by AgentSettings.
    if (payload.status === 'installed') {
      void refreshManagedAgentCatalogAndAssistants();
    }
  }, []);

  const fetchAgents = useCallback(async () => {
    setLoading(true);
    setError(undefined);
    try {
      const extensionList = await ipcBridge.hub.getExtensionList.invoke();
      if (extensionList) {
        // Filter agents
        const agentExtensions = extensionList.filter((ext: IHubAgentItem) => ext.hubs?.includes('acpAdapters'));
        setAgents(agentExtensions);
      }
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchAgents();

    // Listen to state changes from backend
    const unsubscribe = ipcBridge.hub.onStateChanged.on((payload) => {
      applyStateChange(payload);
    });

    return () => {
      unsubscribe();
    };
  }, [applyStateChange, fetchAgents]);

  const install = async (name: string) => {
    try {
      const result = await ipcBridge.hub.install.invoke({ name });
      applyStateChange(result);
    } catch (err) {
      console.error('Install failed:', err);
      // Wait for IPC status update to catch the error and reflect it in UI
    }
  };

  const retryInstall = async (name: string) => {
    try {
      const result = await ipcBridge.hub.retryInstall.invoke({ name });
      applyStateChange(result);
    } catch (err) {
      console.error('Retry failed:', err);
    }
  };

  const update = async (name: string) => {
    try {
      const result = await ipcBridge.hub.update.invoke({ name });
      applyStateChange(result);
    } catch (err) {
      console.error('Update failed:', err);
    }
  };

  return {
    agents,
    loading,
    error,
    refresh: fetchAgents,
    install,
    retryInstall,
    update,
  };
}
