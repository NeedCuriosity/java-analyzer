//- /Messy.java
class Messy {
  int   count;
        String name ;

  int total(int a,int b){
      if(a>b){
    return a ;
      }else{
          for(int i=0;i<b;i++){
      a+=i;
          }
      }


      return a+b;
  }
}
//- /Messy.java formatted
class Messy {
    int count;
    String name;

    int total(int a, int b) {
        if (a > b) {
            return a;
        } else {
            for (int i = 0; i < b; i++) {
                a += i;
            }
        }

        return a + b;
    }
}
