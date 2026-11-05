import { getApplicationClient } from "~/shared/application/client";
import { useIndexQuery } from "~/shared/index-query";
import { type Result } from "~/types/tauri.gen";

export type Person = {
  id: string;
  name: string;
};

const peopleQueryKey = ["people"];

function unwrap<T>(result: Result<T, string>): T {
  if (result.status === "error") {
    throw new Error(result.error);
  }
  return result.data;
}

export async function listPeople(): Promise<Person[]> {
  const items = unwrap(await getApplicationClient().store.peopleList());
  return items.map((item) => ({ id: item.id, name: item.name ?? item.id }));
}

export function usePeople(): Person[] {
  const { data = EMPTY_PEOPLE } = useIndexQuery({
    entity: "people",
    queryKey: peopleQueryKey,
    queryFn: listPeople,
  });

  return data;
}

export async function ensurePerson(name: string): Promise<Person> {
  const item = unwrap(await getApplicationClient().store.peopleEnsure(name));
  return { id: item.id, name: item.name ?? item.id };
}

const EMPTY_PEOPLE: Person[] = [];
