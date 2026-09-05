# Megaman

<!-- impeccable:product-schema 1 -->

## Platform

Desktop application with a shared Rust application core and native frontends: SwiftUI/AppKit on macOS, WinUI on Windows, and GTK4 or Qt on Linux. The user wants native platform components and OS file-management integration.

## Users and purpose

A file manager for daily file operations and development work. The user wants separate Storage and Projects workspaces for files, storage, Git, Rust builds, and npm/Bun projects.

## Required workflows

Browse and search indexed files; drag files and select groups; batch rename, copy, move, compress, and extract; inspect folder sizes, disk capacity, and a linked storage chart; inspect Git changes and manage project build artifacts. Projects match local repositories and worktrees to the active GitHub CLI account. Ctrl+G searches globally.

## Constraints

Preserve working operations and native keyboard, drag, selection, and resizing behavior. Keep expensive work off the interface thread. Distinguish indexed or cached values from live measurements. The user requests a full visual redesign.

## Visual references

The user names macOS Tahoe Finder, File Pilot, Dolphin, and Nautilus as references, particularly their use of color. Preserve a capable native desktop feel with clear navigation, compact file information, and useful inspection panels.

The user also names Spacedrive and T3 Code as references for indexed libraries and diff review, and Files (files.community) for file-manager interactions. On macOS, use current Apple components and Liquid Glass where supported; on Windows, use genuine WinUI controls.
