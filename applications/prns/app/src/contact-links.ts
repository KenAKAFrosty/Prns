import { useCallback, useEffect, useRef, useState } from "react";
import { Linking } from "react-native";
import { contactInputBudget } from "./contact-input";

export type ContactLink = Readonly<{ id: number; url: string }>;
const capacity = 4;
const recentCapacity = 16;

/** One bounded presentation inbox, retained while installation startup/close
 * changes the surrounding views. Links are untrusted input, never commands.
 * This is deliberately not crash-persistent operation or contact storage. */
export function useContactLinks() {
  const [links, setLinks] = useState<readonly ContactLink[]>([]);
  const [notice, setNotice] = useState<string>();
  const [initialError, setInitialError] = useState<string>();
  const current = useRef<readonly ContactLink[]>([]);
  const next = useRef(1);
  const alive = useRef(false);
  const generation = useRef(0);
  const recent = useRef(new Map<string, number>());
  const duringInitial = useRef(new Set<string>());
  const initialPending = useRef(false);
  const receive = useCallback((url: string) => {
    // Match only the registered scheme; do not parse/normalize or fetch URLs.
    if (!/^rns-contact:/i.test(url) || !alive.current) return;
    if (url.length > contactInputBudget) {
      setNotice("An incoming contact link was too large. Your current review was kept. Ask for a new contact link.");
      return;
    }
    const now = Date.now();
    if (current.current.some((link) => link.url === url) || now - (recent.current.get(url) ?? -Infinity) < 2000) return;
    if (current.current.length === capacity) {
      setNotice("Four contact links are already waiting. Finish or dismiss one, then open the extra link again. Your current review was kept.");
      return;
    }
    recent.current.delete(url); recent.current.set(url, now);
    if (recent.current.size > recentCapacity) recent.current.delete(recent.current.keys().next().value!);
    current.current = [...current.current, { id: next.current++, url }];
    setLinks(current.current);
  }, []);
  const readInitial = useCallback(() => {
    if (!alive.current || initialPending.current) return;
    const ticket = ++generation.current;
    initialPending.current = true;
    setInitialError(undefined);
    void (async () => Linking.getInitialURL())().then((url) => {
      if (!alive.current || ticket !== generation.current) return;
      if (typeof url === "string" && !duringInitial.current.has(url)) receive(url);
    }).catch(() => {
      if (alive.current && ticket === generation.current) setInitialError("The opening contact link could not be checked. Retry, or paste the link in Contacts.");
    }).finally(() => {
      if (ticket === generation.current) { initialPending.current = false; duringInitial.current.clear(); }
    });
  }, [receive]);
  useEffect(() => {
    alive.current = true;
    // Subscribe first so a URL delivered during the initial read cannot be lost.
    const subscription = Linking.addEventListener("url", ({ url }) => {
      if (initialPending.current && /^rns-contact:/i.test(url) && url.length <= contactInputBudget) {
        duringInitial.current.add(url);
        if (duringInitial.current.size > recentCapacity) duringInitial.current.delete(duringInitial.current.values().next().value!);
      }
      receive(url);
    });
    readInitial();
    return () => { alive.current = false; initialPending.current = false; generation.current++; subscription.remove(); };
  }, [readInitial, receive]);
  const dismiss = useCallback((id: number) => {
    // A late save of A must never dismiss the next link B.
    if (!alive.current || current.current[0]?.id !== id) return;
    current.current = current.current.slice(1); setLinks(current.current);
  }, []);
  return { current: links[0], waiting: Math.max(0, links.length - 1), notice, initialError,
    retryInitial: readInitial, dismiss, clearNotice: () => setNotice(undefined) };
}
