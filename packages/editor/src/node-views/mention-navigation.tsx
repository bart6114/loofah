import { createContext, useContext } from "react";

export type MentionNavigation = (mention: {
  id: string;
  type: string;
  label: string;
}) => void;

const legacyMentionNavigation: MentionNavigation = ({ id, type }) => {
  const navigate = (window as any).__HYPR_NAVIGATE__;
  if (navigate) navigate(`/app/${type}/${id}`);
};

export const MentionNavigationContext = createContext<MentionNavigation>(
  legacyMentionNavigation,
);
export function useMentionNavigation() {
  return useContext(MentionNavigationContext);
}
