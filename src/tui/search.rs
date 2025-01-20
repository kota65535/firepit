// use std::collections::HashSet;
// 
// #[derive(Debug, Clone)]
// pub struct SearchResults {
//     query: String,
//     // We use Rc<str> instead of String here for two reasons:
//     // - Rc for cheap clones since elements in `matches` will always be in `tasks` as well
//     // - Rc<str> implements Borrow<str> meaning we can query a `HashSet<Rc<str>>` using a `&str`
//     // We do not modify the provided task names so we do not need the capabilities of String.
//     tasks: Vec<String>,
//     matches: HashSet<String>,
// }
// 
// impl SearchResults {
//     pub fn new(tasks: Vec<String>) -> Self {
//         Self {
//             tasks: tasks,
//             query: String::new(),
//             matches: HashSet::new(),
//         }
//     }
// 
// 
//     /// Updates the query and the matches
//     pub fn modify_query(&mut self, modification: impl FnOnce(&mut String)) {
//         modification(&mut self.query);
//         self.update_matches();
//     }
// 
//     fn update_matches(&mut self) {
//         self.matches.clear();
//         if self.query.is_empty() {
//             return;
//         }
//         for task in self.tasks.iter().filter(|task| task.contains(&self.query)) {
//             self.matches.insert(task.clone());
//         }
//     }
// 
//     /// Given an iterator it returns the first task that is in the search
//     /// results
//     pub fn first_match<'a>(&self, mut tasks: impl Iterator<Item = &'a str>) -> Option<&'a str> {
//         tasks.find(|task| self.matches.contains(*task))
//     }
// 
//     /// Returns if there are any matches for the query
//     pub fn has_matches(&self) -> bool {
//         !self.matches.is_empty()
//     }
// 
//     /// Returns query
//     pub fn query(&self) -> &str {
//         &self.query
//     }
// }
