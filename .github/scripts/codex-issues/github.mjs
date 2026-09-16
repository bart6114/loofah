import { repository } from "./core.mjs";

export class GitHub {
  constructor(token = process.env.GH_TOKEN) {
    if (!token) throw new Error("Missing GitHub credential");
    this.token = token;
  }

  async request(path, method = "GET", body) {
    const response = await fetch(
      `https://api.github.com/repos/${repository}/${path}`,
      {
        method,
        headers: {
          Authorization: `Bearer ${this.token}`,
          Accept: "application/vnd.github+json",
          "X-GitHub-Api-Version": "2022-11-28",
          "Content-Type": "application/json",
        },
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: AbortSignal.timeout(30000),
      },
    );
    if (!response.ok)
      throw new Error(
        `GitHub ${method} ${path.split("?")[0]}: ${response.status}`,
      );
    return response.status === 204 ? null : response.json();
  }

  async list(path) {
    const items = [];
    for (let page = 1; page <= 100; page++) {
      const batch = await this.request(
        `${path}${path.includes("?") ? "&" : "?"}per_page=100&page=${page}`,
      );
      items.push(...batch);
      if (batch.length < 100) return items;
    }
    throw new Error("GitHub pagination limit reached");
  }

  async thread(number) {
    const [issue, comments] = await Promise.all([
      this.request(`issues/${number}`),
      this.list(`issues/${number}/comments`),
    ]);
    return { issue, comments };
  }

  comment(number, body) {
    return this.request(`issues/${number}/comments`, "POST", { body });
  }
}
