use chrono::{TimeZone, Utc};
use git2::{Repository, BlameOptions, Oid};
use rayon::prelude::*;
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
    pub folder_path: String,
    pub primary_author: String,
    pub ownership_percentage: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FolderComplexity {
    pub folder_path: String,
    pub complexity_score: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RepositoryMetrics {
    pub commits: Vec<CommitMetric>,
    pub branch_lifespans: Vec<BranchLifespan>,
    pub global_hotspots: Vec<HotspotMetric>,
    pub author_hotspots: HashMap<String, Vec<HotspotMetric>>,
    pub knowledge_silos: Vec<KnowledgeSilo>,
    pub folder_complexities: Vec<FolderComplexity>,
}

pub fn analyze_repository(repo_path: &str) -> Result<RepositoryMetrics, git2::Error> {
    let repo = Repository::open(repo_path)?;
    let mut revwalk = repo.revwalk()?;
    revwalk.push_head()?;

    let oids: Result<Vec<Oid>, git2::Error> = revwalk.collect();
    let oids = oids?;

    struct ChunkResult {
        commits: Vec<CommitMetric>,
        branch_lifespans: Vec<BranchLifespan>,
        global_file_mods: HashMap<String, usize>,
        author_file_mods: HashMap<String, HashMap<String, usize>>,
    }

    let chunk_size = (oids.len() / rayon::current_num_threads()).max(1);

    let chunk_results: Vec<ChunkResult> = oids
        .par_chunks(chunk_size)
        .filter_map(|chunk| {
            let thread_repo = Repository::open(repo_path).ok()?;
            let mut chunk_commits = Vec::new();
            let mut chunk_lifespans = Vec::new();
            let mut chunk_global_mods = HashMap::new();
            let mut chunk_author_mods: HashMap<String, HashMap<String, usize>> = HashMap::new();

            for &oid in chunk {
                let commit = match thread_repo.find_commit(oid) {
                    Ok(c) => c,
                    Err(_) => continue,
                };

                let author = commit.author().name().unwrap_or("Unknown").to_string();
                let timestamp = commit.time().seconds();
                let date = Utc.timestamp_opt(timestamp, 0).unwrap().to_rfc3339();

                let mut lines_added = 0;
                let mut lines_deleted = 0;

                let parents: Vec<_> = commit.parents().collect();
                let parent_tree = if parents.len() > 0 {
                    parents[0].tree().ok()
                } else {
                    None
                };
                let commit_tree = match commit.tree() {
                    Ok(t) => t,
                    Err(_) => continue,
                };

                let diff = thread_repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&commit_tree), None).ok();

                let mut modified_files_in_commit = std::collections::HashSet::new();

                if let Some(diff) = diff {
                    let _ = diff.print(git2::DiffFormat::Patch, |delta, _hunk, line| {
                        if let Some(path) = delta.new_file().path().and_then(|p| p.to_str()) {
                            modified_files_in_commit.insert(path.to_string());
                        }

                        match line.origin() {
                            '+' => lines_added += 1,
                            '-' => lines_deleted += 1,
                            _ => {}
                        }
                        true
                    });
                }

                for path_str in modified_files_in_commit {
                    *chunk_global_mods.entry(path_str.clone()).or_insert(0) += 1;
                    *chunk_author_mods
                        .entry(author.clone())
                        .or_default()
                        .entry(path_str)
                        .or_insert(0) += 1;
                }

                chunk_commits.push(CommitMetric {
                    hash: oid.to_string(),
                    author: author.clone(),
                    timestamp,
                    date,
                    lines_added,
                    lines_deleted,
                });

                if parents.len() > 1 {
                    if let Ok(base_oid) = thread_repo.merge_base(parents[0].id(), parents[1].id()) {
                        if let Ok(base_commit) = thread_repo.find_commit(base_oid) {
                            let duration = commit.time().seconds() - base_commit.time().seconds();
                            chunk_lifespans.push(BranchLifespan {
                                merge_commit: oid.to_string(),
                                author: author.clone(),
                                duration_seconds: duration,
                            });
                        }
                    }
                }
            }

            Some(ChunkResult {
                commits: chunk_commits,
                branch_lifespans: chunk_lifespans,
                global_file_mods: chunk_global_mods,
                author_file_mods: chunk_author_mods,
            })
        })
        .collect();

    let mut commits = Vec::new();
    let mut branch_lifespans = Vec::new();
    let mut global_file_mods: HashMap<String, usize> = HashMap::new();
    let mut author_file_mods: HashMap<String, HashMap<String, usize>> = HashMap::new();

    for mut res in chunk_results {
        commits.append(&mut res.commits);
        branch_lifespans.append(&mut res.branch_lifespans);

        for (path, count) in res.global_file_mods {
            *global_file_mods.entry(path).or_insert(0) += count;
        }

        for (author, mods) in res.author_file_mods {
            let author_entry = author_file_mods.entry(author).or_default();
            for (path, count) in mods {
                *author_entry.entry(path).or_insert(0) += count;
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

    // Knowledge Silos by Folder via blame
    let mut knowledge_silos = Vec::new();
    if let Ok(head) = repo.head() {
        if let Ok(tree) = head.peel_to_tree() {
            let mut paths = Vec::new();
            tree.walk(git2::TreeWalkMode::PreOrder, |root, entry| {
                if entry.kind() == Some(git2::ObjectType::Blob) {
                    if let Some(name) = entry.name() {
                        paths.push((root.to_string(), format!("{}{}", root, name)));
                    }
                }
                git2::TreeWalkResult::Ok
            }).unwrap_or(());

            let paths_chunk_size = (paths.len() / rayon::current_num_threads()).max(1);

            let chunked_folder_stats: Vec<HashMap<String, (usize, HashMap<String, usize>)>> = paths
                .par_chunks(paths_chunk_size)
                .filter_map(|chunk| {
                    let thread_repo = Repository::open(repo_path).ok()?;
                    let mut chunk_folder_stats: HashMap<String, (usize, HashMap<String, usize>)> = HashMap::new();

                    for (_root_dir, path) in chunk {
                        let mut blame_opts = BlameOptions::new();
                        if let Ok(blame) = thread_repo.blame_file(Path::new(path), Some(&mut blame_opts)) {
                            let mut file_author_lines: HashMap<String, usize> = HashMap::new();
                            let mut file_total_lines = 0;

                            for hunk in blame.iter() {
                                let author = hunk.final_signature().name().unwrap_or("Unknown").to_string();
                                let lines = hunk.lines_in_hunk();
                                *file_author_lines.entry(author).or_insert(0) += lines;
                                file_total_lines += lines;
                            }

                            if file_total_lines > 0 {
                                let mut current_dir = Path::new(path).parent().unwrap_or(Path::new(""));
                                loop {
                                    let dir_str = current_dir.to_str().unwrap_or("");
                                    let folder_key = if dir_str.is_empty() { ".".to_string() } else { dir_str.to_string() };

                                    let entry = chunk_folder_stats.entry(folder_key).or_insert_with(|| (0, HashMap::new()));
                                    entry.0 += file_total_lines;
                                    for (author, lines) in &file_author_lines {
                                        *entry.1.entry(author.clone()).or_insert(0) += lines;
                                    }

                                    if let Some(parent) = current_dir.parent() {
                                        if current_dir == Path::new("") {
                                            break;
                                        }
                                        current_dir = parent;
                                    } else {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    Some(chunk_folder_stats)
                })
                .collect();

            let mut final_folder_stats: HashMap<String, (usize, HashMap<String, usize>)> = HashMap::new();
            for chunk_stats in chunked_folder_stats {
                for (folder_key, (total_lines, author_lines)) in chunk_stats {
                    let entry = final_folder_stats.entry(folder_key).or_insert_with(|| (0, HashMap::new()));
                    entry.0 += total_lines;
                    for (author, lines) in author_lines {
                        *entry.1.entry(author).or_insert(0) += lines;
                    }
                }
            }

            for (folder, (total_lines, author_lines)) in final_folder_stats {
                if total_lines > 0 {
                    for (author, lines) in author_lines {
                        let percentage = (lines as f64) / (total_lines as f64);
                        if percentage >= 0.95 {
                            knowledge_silos.push(KnowledgeSilo {
                                folder_path: folder.clone(),
                                primary_author: author,
                                ownership_percentage: percentage * 100.0,
                            });
                            break;
                        }
                    }
                }
            }
        }
    }

    let folder_complexities = calculate_folder_complexity(repo_path);

    Ok(RepositoryMetrics {
        commits,
        branch_lifespans,
        global_hotspots,
        author_hotspots: author_hotspots_sorted,
        knowledge_silos,
        folder_complexities,
    })
}

fn calculate_folder_complexity(repo_path: &str) -> Vec<FolderComplexity> {
    let walker = walkdir::WalkDir::new(repo_path).into_iter();

    let mut files_to_process = Vec::new();

    for entry in walker.filter_entry(|e| !e.path().to_string_lossy().contains(".git")) {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        if entry.file_type().is_file() {
            let path = entry.path();
            if let Some(ext) = path.extension() {
                let ext_str = ext.to_string_lossy();
                if ext_str == "cs" || ext_str == "js" || ext_str == "ts" || ext_str == "jsx" || ext_str == "tsx" || ext_str == "rs" {
                    files_to_process.push(path.to_path_buf());
                }
            }
        }
    }

    let folder_scores_chunks: Vec<HashMap<String, usize>> = files_to_process
        .par_iter()
        .filter_map(|path| {
            if let Ok(content) = std::fs::read_to_string(path) {
                let score = calculate_heuristic_complexity(&content);
                if score > 0 {
                    let mut chunk_scores = HashMap::new();
                    let rel_path = path.strip_prefix(repo_path).unwrap_or(path);
                    let mut current_dir = rel_path.parent().unwrap_or(Path::new(""));

                    loop {
                        let dir_str = current_dir.to_str().unwrap_or("");
                        let folder_key = if dir_str.is_empty() { ".".to_string() } else { dir_str.to_string() };

                        *chunk_scores.entry(folder_key).or_insert(0) += score;

                        if let Some(parent) = current_dir.parent() {
                            if current_dir == Path::new("") {
                                break;
                            }
                            current_dir = parent;
                        } else {
                            break;
                        }
                    }
                    Some(chunk_scores)
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect();

    let mut final_folder_scores: HashMap<String, usize> = HashMap::new();
    for chunk in folder_scores_chunks {
        for (folder, score) in chunk {
            *final_folder_scores.entry(folder).or_insert(0) += score;
        }
    }

    let mut complexities: Vec<FolderComplexity> = final_folder_scores
        .into_iter()
        .map(|(folder_path, complexity_score)| FolderComplexity {
            folder_path,
            complexity_score,
        })
        .collect();

    complexities.sort_by(|a, b| b.complexity_score.cmp(&a.complexity_score));
    complexities
}

fn calculate_heuristic_complexity(content: &str) -> usize {
    let mut score = 0;

    // Very basic regex-based keyword matching that tries to avoid comments and strings
    // In a production scenario, you'd want a slightly more robust state machine or regex
    let keywords = [
        "if", "while", "for", "switch", "case", "catch", "match"
    ];

    let mut in_block_comment = false;

    for line in content.lines() {
        let mut trimmed = line.trim();

        // Handle block comments (simple heuristic)
        if in_block_comment {
            if let Some(end_idx) = trimmed.find("*/") {
                in_block_comment = false;
                trimmed = &trimmed[end_idx + 2..];
            } else {
                continue;
            }
        } else if let Some(start_idx) = trimmed.find("/*") {
            in_block_comment = true;
            trimmed = &trimmed[..start_idx];
        }

        // Handle line comments
        if let Some(idx) = trimmed.find("//") {
            trimmed = &trimmed[..idx];
        }

        // Count operators (&&, ||, ?)
        score += trimmed.matches("&&").count();
        score += trimmed.matches("||").count();
        score += trimmed.matches("?").count();

        // Count keywords
        // We use split_whitespace and check if the keyword exactly matches
        // or starts with it followed by '(' to avoid partial matches like 'shift' or 'formatting'
        for token in trimmed.split(|c: char| !c.is_alphanumeric() && c != '_') {
            if keywords.contains(&token) {
                score += 1;
            }
        }
    }

    score
}
