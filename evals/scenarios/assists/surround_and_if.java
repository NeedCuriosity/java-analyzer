class Main {
    void work(int n) {
        /*@s1*/System.out.println(n);
        System.out.println(n + 1);/*@s2*/
        int /*@d1*/x = n;/*@d2*/
        System.out.println(x);
    }

    static void lock() {
        /*@l1*/System.out.println("locked");/*@l2*/
    }

    void split(String s) {
        /*@split*/if (s != null && !s.isEmpty()) {
            System.out.println(s);
        }
    }

    void merge(String s) {
        /*@merge*/if (s != null) {
            if (s.isEmpty() || s.isBlank()) {
                System.out.println(s);
            }
        }
    }

    void noMerge(String s) {
        /*@no_merge*/if (s != null) {
            if (s.isEmpty()) {
                System.out.println(s);
            } else {
                System.out.println("x");
            }
        }
    }
}
//? assist surround_if s1 s2 ; has "        if (true) {\n            System.out.println(n);\n            System.out.println(n + 1);\n        }"
//? assist surround_try_catch s1 s2 ; has "        } catch (Exception e) {\n            throw new RuntimeException(e);\n        }"
//? assist surround_synchronized s1 s2 ; has "synchronized (this) {"
//? assist surround_synchronized l1 l2 ; has "synchronized (Main.class) {"
//? noassist surround_if d1 d2
//? assist split_if split ; has "        if (s != null) {\n            if (!s.isEmpty()) {\n                System.out.println(s);\n            }\n        }"
//? assist merge_nested_if merge ; has "        if (s != null && (s.isEmpty() || s.isBlank())) {\n            System.out.println(s);\n        }"
//? noassist merge_nested_if no_merge
