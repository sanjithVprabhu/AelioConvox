const chains = new Map<string, Promise<void>>();

/** Serialize async work per customer key (e.g. `whatsapp:+1555`). */
export function enqueueCustomerTurn(customerKey: string, fn: () => Promise<void>): void {
  const previous = chains.get(customerKey) ?? Promise.resolve();
  const next = previous
    .then(fn)
    .catch(() => {})
    .finally(() => {
      if (chains.get(customerKey) === next) {
        chains.delete(customerKey);
      }
    });
  chains.set(customerKey, next);
}
