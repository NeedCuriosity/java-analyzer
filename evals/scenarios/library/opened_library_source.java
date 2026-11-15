//- /org/lib/Repository.java lib:core open
package org.lib;

import java.util.ArrayList;
import java.util.List;

public class Repository {
    private final List<String> /*@items*/items = new ArrayList<>();
    private final Helper /*@helper*/helper = new Helper();

    public /*@ctor*/Repository() {}

    public int /*@add*/add(String /*@param*/name) {
        String /*@local*/trimmed = /*@call_normalize*/normalize(/*@use_param*/name);
        /*@use_items*/items.add(/*@use_local2*/trimmed);
        Helper.Inner inner = /*@use_helper*/helper./*@call_make*/make();
        return /*@use_local*/trimmed.length() + inner./*@use_count*/count + /*@call_size*/size();
    }

    public int /*@size*/size() { return items./*@list_size*/size(); }

    public int add(String name, int times) { return times * /*@inner_add*/add(name); }

    private String /*@normalize*/normalize(String s) { return s.trim(); }

    static class Helper {
        Inner /*@make*/make() { return new Inner(); }

        static class Inner { int /*@count*/count; }
    }
}
//- /app/Main.java
package app;

import org.lib.Repository;

class Main {
    int run() {
        Repository repo = new /*@new_repo*/Repository();
        return repo./*@main_add*/add("x") + repo./*@main_add2*/add("y", 2) + repo./*@main_size*/size();
    }
}
//? def call_normalize -> normalize
//? def use_param -> param
//? def use_local -> local
//? def use_items -> items
//? def use_helper -> helper
//? def call_make -> make
//? def use_count -> count
//? def call_size -> size
//? def list_size -> lib java.util.List#size
//? hover call_normalize ~ "normalize(String s)"
//? hover use_local ~ "String trimmed"
//? refs local = use_local2 use_local
//? refs param = use_param
//? refs normalize = call_normalize
//? refs add = inner_add main_add
//? refs size = call_size main_size
//? refs ctor = new_repo
//? def main_add -> add
//? def main_size -> size
//? diag none
