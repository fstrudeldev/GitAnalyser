use chrono::{TimeZone, Utc};
use git2::{Repository, BlameOptions};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CommitMetric {
    pub hash: String,
    pub author: String,
    pub timestamp: i64,
    pub date: String,
    pub lines_added: usize,
    pub lines_deleted: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BranchLifespan {
    pub merge_commit: String,
    pub author: String,
    pub duration_seconds: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HotspotMetric {
    pub file_path: String,
    pub modifications: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct KnowledgeSilo {
    pub file_path: String,
    pub primary_author: String,
    pub ownership_percentage: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RepositoryMetrics {
    pub commits: Vec<CommitMetric>,
    pub branch_lifespans: Vec<BranchLifespan>,
    pub global_hotspots: Vec<HotspotMetric>,
    pub author_hotspots: HashMap<String, Vec<HotspotMetric>>,
    pub knowledge_silos: Vec<KnowledgeSilo>,
}

pub fn analyze_repository(repo_path: &str) -> Result<RepositoryMetrics, git2::Error> {
    let repo = Repository::open(repo_path)?;
    let mut revwalk = repo.revwalk()?;
    revwalk.push_head()?;

    let mut commits = Vec::new();
    let mut branch_lifespans = Vec::new();

    let mut global_file_mods: HashMap<String, usize> = HashMap::new();
    let mut author_file_mods: HashMap<String, HashMap<String, usize>> = HashMap::new();

    for oid in revwalk {
        let oid = oid?;
        let commit = repo.find_commit(oid)?;

        let author = commit.author().name().unwrap_or("Unknown").to_string();
        let timestamp = commit.time().seconds();
        let date = Utc.timestamp_opt(timestamp, 0).unwrap().to_rfc3339();

        let mut lines_added = 0;
        let mut lines_deleted = 0;

        let parents: Vec<_> = commit.parents().collect();
        let parent_tree = if parents.len() > 0 {
            Some(parents[0].tree()?)
        } else {
            None
        };
        let commit_tree = commit.tree()?;

        let diff = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&commit_tree), None)?;

        // Track which files were modified in this specific commit to avoid overcounting
        let mut modified_files_in_commit = std::collections::HashSet::new();

        diff.print(git2::DiffFormat::Patch, |delta, _hunk, line| {
            if let Some(path) = delta.new_file().path().and_then(|p| p.to_str()) {
                modified_files_in_commit.insert(path.to_string());
            }

            match line.origin() {
                '+' => lines_added += 1,
                '-' => lines_deleted += 1,
                _ => {}
            }
            true
        })?;

        for path_str in modified_files_in_commit {
            *global_file_mods.entry(path_str.clone()).or_insert(0) += 1;
            *author_file_mods
                .entry(author.clone())
                .or_default()
                .entry(path_str)
                .or_insert(0) += 1;
        }

        commits.push(CommitMetric {
            hash: oid.to_string(),
            author: author.clone(),
            timestamp,
            date,
            lines_added,
            lines_deleted,
        });

        // Branch lifespan calculation (merge commits)
        if parents.len() > 1 {
            // Very simplified branch lifespan estimation
            // The oldest commit in the merged branch relative to the main branch
            // This requires finding the merge base, which can be complex.
            // For now, we will just find the oldest commit that is reachable from parent[1] but not parent[0].
            if let Ok(base_oid) = repo.merge_base(parents[0].id(), parents[1].id()) {
                if let Ok(base_commit) = repo.find_commit(base_oid) {
                    let duration = commit.time().seconds() - base_commit.time().seconds();
                    branch_lifespans.push(BranchLifespan {
                        merge_commit: oid.to_string(),
                        author: author.clone(),
                        duration_seconds: duration,
                    });
                }
            }
        }
    }

    let mut global_hotspots: Vec<HotspotMetric> = global_file_mods
        .into_iter()
        .map(|(path, mods)| HotspotMetric { file_path: path, modifications: mods })
        .collect();
    global_hotspots.sort_by(|a, b| b.modifications.cmp(&a.modifications));

    let mut author_hotspots_sorted = HashMap::new();
    for (author, mods) in author_file_mods {
        let mut hotspots: Vec<HotspotMetric> = mods
            .into_iter()
            .map(|(path, mods)| HotspotMetric { file_path: path, modifications: mods })
            .collect();
        hotspots.sort_by(|a, b| b.modifications.cmp(&a.modifications));
        author_hotspots_sorted.insert(author, hotspots);
    }

    // Knowledge Silos via blame
    let mut knowledge_silos = Vec::new();
    if let Ok(head) = repo.head() {
        if let Ok(tree) = head.peel_to_tree() {
            let mut paths = Vec::new();
            tree.walk(git2::TreeWalkMode::PreOrder, |root, entry| {
                if let Some(name) = entry.name() {
                    paths.push(format!("{}{}", root, name));
                }
                git2::TreeWalkResult::Ok
            }).unwrap_or(());

            for path in paths {
                // Ignore errors for binary files or files not suited for blame
                let mut blame_opts = BlameOptions::new();
                if let Ok(blame) = repo.blame_file(Path::new(&path), Some(&mut blame_opts)) {
                    let mut author_lines: HashMap<String, usize> = HashMap::new();
                    let mut total_lines = 0;

                    for hunk in blame.iter() {
                        let author = hunk.final_signature().name().unwrap_or("Unknown").to_string();
                        let lines = hunk.lines_in_hunk();
                        *author_lines.entry(author).or_insert(0) += lines;
                        total_lines += lines;
                    }

                    if total_lines > 0 {
                        for (author, lines) in author_lines {
                            let percentage = (lines as f64) / (total_lines as f64);
                            if percentage >= 0.95 {
                                knowledge_silos.push(KnowledgeSilo {
                                    file_path: path.clone(),
                                    primary_author: author,
                                    ownership_percentage: percentage * 100.0,
                                });
                                break; // Only one author can have >= 95%
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(RepositoryMetrics {
        commits,
        branch_lifespans,
        global_hotspots,
        author_hotspots: author_hotspots_sorted,
        knowledge_silos,
    })
}
