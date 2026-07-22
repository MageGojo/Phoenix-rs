export interface Member {
  id: number;
  name: string;
  email: string;
  city: string;
  role: string;
  status: "active" | "away" | "offline";
  projects: number;
  joinedOn: string;
  lastActiveMinutes: number;
  createdBy?: "Rust";
}
