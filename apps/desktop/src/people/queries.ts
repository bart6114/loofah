import { useQuery } from "@tanstack/react-query";

import { getApplicationClient } from "~/shared/application/client";
import type { PersonItem } from "~/shared/application/protocol.gen";
import { useVaultReadScope } from "~/shared/application/use-vault-read-scope";
import { type Result } from "~/types/tauri.gen";

export type Person = {
  id: string;
  name: string;
};

function unwrap<T>(result: Result<T, string>): T {
  if (result.status === "error") {
    throw new Error(result.error);
  }
  return result.data;
}

export async function listPeople(): Promise<Person[]> {
  const items = unwrap(await getApplicationClient().store.peopleList());
  return peopleRecords(items);
}

export function usePeople(): Person[] {
  const owner = useVaultReadScope();
  const supported = owner.scope.client.features.includes("content_read");
  const query = useQuery(
    {
      ...owner.scope.peopleOptions(),
      enabled: supported && owner.readable,
      select: peopleRecords,
    },
    owner.scope.queryClient,
  );
  return supported && owner.readable
    ? (query.data ?? EMPTY_PEOPLE)
    : EMPTY_PEOPLE;
}
function peopleRecords(items: PersonItem[]): Person[] {
  return items.map((item) => ({ id: item.id, name: item.name ?? item.id }));
}

export async function ensurePerson(name: string): Promise<Person> {
  const item = unwrap(await getApplicationClient().store.peopleEnsure(name));
  return { id: item.id, name: item.name ?? item.id };
}

const EMPTY_PEOPLE: Person[] = [];
