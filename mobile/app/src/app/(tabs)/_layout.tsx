import { Tabs } from 'expo-router';

// Keep each tab's view and local state when another tab is selected. Replacing
// stack screens rebuilt Worktrees empty, then reloaded its desktop and folds,
// moving the list on every return. The screens draw the shared floating bar.
export default function TabLayout() {
  return (
    <Tabs backBehavior="none" screenOptions={{ headerShown: false, animation: 'none' }} tabBar={() => null}>
      <Tabs.Screen name="index" />
      <Tabs.Screen name="usage" />
      <Tabs.Screen name="needs" />
      <Tabs.Screen name="settings" />
    </Tabs>
  );
}
