import { useQuery } from "@tanstack/react-query";

import type { TagSuggestionsRequest } from "~/shared/application/protocol.gen";
import type { VaultScope } from "~/shared/application/vault-scope";

export function useTagSuggestions(
  scope: VaultScope,
  input: TagSuggestionsRequest,
  enabled: boolean,
) {
  return useQuery(
    { ...scope.tagSuggestionsOptions(input), enabled },
    scope.queryClient,
  );
}
