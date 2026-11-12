/** How long a card that has just appeared ignores its buttons, so a click meant for the card before it cannot land on it. */
let delay = 500;
export const armDelay = () => delay;
/** For tests. */
export const setArmDelay = (ms: number) => {
  delay = ms;
};
