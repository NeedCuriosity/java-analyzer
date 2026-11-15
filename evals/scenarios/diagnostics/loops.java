import java.util.List;

class Main {
    void stuck(int n) {
        int i = 0;
        while (/*@stuck*/i < n) {
            System.out.println(i);
        }
    }

    void fine(int n, List<String> items) {
        int i = 0;
        while (i < n) {
            i++;
        }
        boolean running = true;
        while (running) {
            if (items.isEmpty()) {
                break;
            }
        }
        int k = 0;
        do {
            k += 2;
        } while (k < n);
        for (int j = 0; j < n; j++) {
            System.out.println(j);
        }
        while (!items.isEmpty()) {
            items.remove(0);
        }
    }

    void stuckFor(int n) {
        for (int j = 0; /*@stuck_for*/j < n; ) {
            System.out.println(j);
        }
    }

    String join(List<String> items) {
        String result = "";
        for (String item : items) {
            /*@concat*/result += item + ",";
        }
        String other = "";
        int i = 0;
        while (i < items.size()) {
            /*@concat2*/other = other + items.get(i);
            i++;
        }
        String date = "";
        for (String item : items) {
            date = item;
            date += "!";
            System.out.println(date);
        }
        for (String item : items) {
            String line = "";
            line += item;
            System.out.println(line);
        }
        return result + other;
    }
}
//? diag loop-condition-not-updated stuck
//? diag loop-condition-not-updated stuck_for
//? diag only loop-condition-not-updated
//? diag string-concat-in-loop concat
//? diag string-concat-in-loop concat2
//? diag only string-concat-in-loop
