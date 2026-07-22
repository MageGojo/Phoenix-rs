import { useState } from "react";

export interface LikeButtonProps {
  initialLikes: number;
}

export default function LikeButton({ initialLikes }: LikeButtonProps) {
  const [likes, setLikes] = useState(initialLikes);
  return <button onClick={() => setLikes((value) => value + 1)}>{likes} likes</button>;
}
